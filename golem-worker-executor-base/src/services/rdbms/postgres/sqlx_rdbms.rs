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

use crate::services::golem_config::{RdbmsConfig, RdbmsPoolConfig};
use crate::services::rdbms::postgres::types::{
    Composite, CompositeType, DbColumn, DbColumnType, DbValue, Domain, DomainType, Enum, EnumType,
    Interval, NamedType, Range, RangeType, TimeTz, ValuesRange,
};
use crate::services::rdbms::postgres::{PostgresType, POSTGRES};
use crate::services::rdbms::sqlx_common::{
    create_db_result, PoolCreator, QueryExecutor, QueryParamsBinder, SqlxDbResultStream, SqlxRdbms,
};
use crate::services::rdbms::{DbResult, DbResultStream, DbRow, Error, Rdbms, RdbmsPoolKey};
use async_trait::async_trait;
use bigdecimal::BigDecimal;
use bit_vec::BitVec;
use futures_util::stream::BoxStream;
use mac_address::MacAddress;
use serde_json::json;
use sqlx::postgres::types::{Oid, PgInterval, PgMoney, PgRange, PgTimeTz};
use sqlx::postgres::{PgConnectOptions, PgTypeKind};
use sqlx::{Column, ConnectOptions, Pool, Row, Type, TypeInfo, ValueRef};
use std::collections::Bound;
use std::fmt::Display;
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
        let options =
            PgConnectOptions::from_url(&self.address).map_err(Error::connection_failure)?;
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(config.max_connections)
            .connect_with(options)
            .await
            .map_err(Error::connection_failure)
    }
}

#[async_trait]
impl QueryExecutor<PostgresType, sqlx::Postgres> for PostgresType {
    async fn execute<'c, E>(
        statement: &str,
        params: Vec<DbValue>,
        executor: E,
    ) -> Result<u64, Error>
    where
        E: sqlx::Executor<'c, Database = sqlx::Postgres>,
    {
        let query: sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments> =
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
    ) -> Result<DbResult<PostgresType>, Error>
    where
        E: sqlx::Executor<'c, Database = sqlx::Postgres>,
    {
        let query: sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments> =
            sqlx::query(statement).bind_params(params)?;
        let result = query
            .fetch_all(executor)
            .await
            .map_err(Error::query_execution_failure)?;
        create_db_result::<PostgresType, sqlx::Postgres>(result)
    }

    async fn query_stream<'c, E>(
        statement: &str,
        params: Vec<DbValue>,
        batch: usize,
        executor: E,
    ) -> Result<Arc<dyn DbResultStream<PostgresType> + Send + Sync + 'c>, Error>
    where
        E: sqlx::Executor<'c, Database = sqlx::Postgres>,
    {
        let query: sqlx::query::Query<sqlx::Postgres, sqlx::postgres::PgArguments> =
            sqlx::query(statement.to_string().leak()).bind_params(params)?;

        let stream: BoxStream<Result<sqlx::postgres::PgRow, sqlx::Error>> = query.fetch(executor);

        let response: SqlxDbResultStream<'c, PostgresType, sqlx::postgres::Postgres> =
            SqlxDbResultStream::create(stream, batch).await?;
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
    let column_type = value.get_type();
    match &column_type {
        DbColumnType::Enum(_) => {
            let v = if let DbValue::Enum(v) = value {
                Some(v)
            } else {
                None
            };
            setter.try_set_value(v)
        }
        DbColumnType::Composite(_) => {
            let v = if let DbValue::Composite(v) = value {
                Some(v)
            } else {
                None
            };
            setter.try_set_value(v)
        }
        DbColumnType::Domain(_) => {
            let v = if let DbValue::Domain(v) = value {
                Some(v)
            } else {
                None
            };
            setter.try_set_value(v)
        }
        DbColumnType::Array(t) => {
            let base_type: DbColumnType = *t.clone();
            match base_type {
                DbColumnType::Enum(_) => {
                    let values: Vec<_> = get_array_plain_values(value, |v| {
                        if let DbValue::Enum(v) = v {
                            Some(v)
                        } else {
                            None
                        }
                    })?;
                    setter.try_set_value(PgEnums(values))
                }
                DbColumnType::Composite(_) => {
                    let values: Vec<_> = get_array_plain_values(value, |v| {
                        if let DbValue::Composite(v) = v {
                            Some(v)
                        } else {
                            None
                        }
                    })?;
                    setter.try_set_value(PgComposites(values))
                }
                DbColumnType::Domain(_) => {
                    let values: Vec<_> = get_array_plain_values(value, |v| {
                        if let DbValue::Domain(v) = v {
                            Some(v)
                        } else {
                            None
                        }
                    })?;
                    setter.try_set_value(PgDomains(values))
                }
                DbColumnType::Range(v) => {
                    set_value_helper(setter, &v.base_type, value, DbValueType::RangeArray)
                }
                _ => set_value_helper(setter, &base_type, value, DbValueType::Array),
            }
        }
        DbColumnType::Range(t) => set_value_helper(setter, &t.base_type, value, DbValueType::Range),
        _ => set_value_helper(setter, &column_type, value, DbValueType::Primitive),
    }
}

fn set_value_helper<'a, S: PgValueSetter<'a>>(
    setter: &mut S,
    column_type: &DbColumnType,
    value: DbValue,
    value_type: DbValueType,
) -> Result<(), String> {
    match column_type {
        DbColumnType::Boolean => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Boolean(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Character => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Character(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Int2 => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Int2(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Int4 => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Int4(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Int8 => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Int8(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Float4 => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Float4(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Float8 => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Float8(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Numeric => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Numeric(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Text => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Text(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Varchar => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Varchar(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Bpchar => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Bpchar(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Bytea => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Bytea(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Uuid => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Uuid(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Json => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Json(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Jsonb => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Jsonb(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Jsonpath => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Jsonpath(v) = v {
                Some(PgJsonPath(v))
            } else {
                None
            }
        }),
        DbColumnType::Xml => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Xml(v) = v {
                Some(PgXml(v))
            } else {
                None
            }
        }),
        DbColumnType::Timestamptz => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Timestamptz(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Timestamp => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Timestamp(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Date => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Date(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Time => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Time(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Timetz => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Timetz(v) = v {
                Some(PgTimeTz::from(v))
            } else {
                None
            }
        }),
        DbColumnType::Interval => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Interval(v) = v {
                Some(PgInterval::from(v))
            } else {
                None
            }
        }),
        DbColumnType::Inet => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Inet(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Cidr => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Cidr(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Macaddr => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Macaddr(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Bit => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Bit(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Varbit => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Varbit(v) = v {
                Some(v)
            } else {
                None
            }
        }),
        DbColumnType::Int4range => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Int4range(v) = v {
                Some(PgRange::from(v))
            } else {
                None
            }
        }),
        DbColumnType::Int8range => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Int8range(v) = v {
                Some(PgRange::from(v))
            } else {
                None
            }
        }),
        DbColumnType::Numrange => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Numrange(v) = v {
                Some(PgRange::from(v))
            } else {
                None
            }
        }),
        DbColumnType::Tsrange => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Tsrange(v) = v {
                Some(PgRange::from(v))
            } else {
                None
            }
        }),
        DbColumnType::Tstzrange => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Tstzrange(v) = v {
                Some(PgRange::from(v))
            } else {
                None
            }
        }),
        DbColumnType::Daterange => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Daterange(v) = v {
                Some(PgRange::from(v))
            } else {
                None
            }
        }),
        DbColumnType::Oid => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Oid(v) = v {
                Some(Oid(v))
            } else {
                None
            }
        }),
        DbColumnType::Money => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Money(v) = v {
                Some(PgMoney(v))
            } else {
                None
            }
        }),
        DbColumnType::Null => setter.try_set_db_value(value, value_type, |v| {
            if let DbValue::Null = v {
                Some(PgNull {})
            } else {
                None
            }
        }),
        _ => Err(format!(
            "{} do not support '{}' value",
            value_type, column_type
        )),
    }
}

fn get_array_plain_values<T>(
    value: DbValue,
    f: impl Fn(DbValue) -> Option<T>,
) -> Result<Vec<T>, String> {
    match value {
        DbValue::Array(vs) => get_plain_values(vs, f),
        v => Err(format!("'{}' is not array", v)),
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

fn get_pg_range<T: Clone>(
    value: Range,
    f: impl Fn(DbValue) -> Option<T> + Clone,
) -> Result<PgCustomRange<T>, String> {
    fn to_value<T: Clone>(v: DbValue, f: impl Fn(DbValue) -> Option<T>) -> Result<T, String> {
        if let Some(v) = f(v.clone()) {
            Ok(v)
        } else {
            Err(format!(
                "Bound element '{}' has different type than expected",
                v
            ))
        }
    }

    fn to_bound<T: Clone>(
        v: Bound<DbValue>,
        f: impl Fn(DbValue) -> Option<T>,
    ) -> Result<Bound<T>, String> {
        match v {
            Bound::Included(v) => Ok(Bound::Included(to_value(v, f)?)),
            Bound::Excluded(v) => Ok(Bound::Excluded(to_value(v, f)?)),
            Bound::Unbounded => Ok(Bound::Unbounded),
        }
    }

    let name = value.name;
    let value = *value.value;

    let start = to_bound(value.start, f.clone())?;
    let end = to_bound(value.end, f.clone())?;

    Ok(PgCustomRange::new(name, PgRange { start, end }))
}

fn get_pg_ranges<T: Clone>(
    values: Vec<Range>,
    f: impl Fn(DbValue) -> Option<T> + Clone,
) -> Result<Vec<PgCustomRange<T>>, String> {
    let mut result: Vec<PgCustomRange<T>> = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let r = get_pg_range(value.clone(), f.clone());

        if let Ok(v) = r {
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

fn get_range<T>(value: PgCustomRange<T>, f: impl Fn(T) -> DbValue + Clone) -> DbValue {
    let name = value.name;
    let start = value.value.start.map(f.clone());
    let end = value.value.end.map(f.clone());
    let value = ValuesRange { start, end };
    DbValue::Range(Range::new(name, value))
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
        DbColumnType::Array(t) => {
            let base_type: DbColumnType = *t.clone();
            match base_type {
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
                DbColumnType::Range(v) => {
                    get_db_value_helper(&v.base_type, DbValueType::RangeArray, getter)?
                }
                _ => get_db_value_helper(&base_type, DbValueType::Array, getter)?,
            }
        }
        DbColumnType::Range(v) => get_db_value_helper(&v.base_type, DbValueType::Range, getter)?,
        _ => get_db_value_helper(db_type, DbValueType::Primitive, getter)?,
    };
    Ok(value)
}

fn get_db_value_helper<G: PgValueGetter>(
    column_type: &DbColumnType,
    value_type: DbValueType,
    getter: &mut G,
) -> Result<DbValue, String> {
    let value = match column_type {
        DbColumnType::Boolean => getter.try_get_db_value::<bool>(value_type, DbValue::Boolean)?,
        DbColumnType::Character => getter.try_get_db_value::<i8>(value_type, DbValue::Character)?,
        DbColumnType::Int2 => getter.try_get_db_value::<i16>(value_type, DbValue::Int2)?,
        DbColumnType::Int4 => getter.try_get_db_value::<i32>(value_type, DbValue::Int4)?,
        DbColumnType::Int8 => getter.try_get_db_value::<i64>(value_type, DbValue::Int8)?,
        DbColumnType::Float4 => getter.try_get_db_value::<f32>(value_type, DbValue::Float4)?,
        DbColumnType::Float8 => getter.try_get_db_value::<f64>(value_type, DbValue::Float8)?,
        DbColumnType::Numeric => {
            getter.try_get_db_value::<BigDecimal>(value_type, DbValue::Numeric)?
        }
        DbColumnType::Uuid => getter.try_get_db_value::<Uuid>(value_type, DbValue::Uuid)?,
        DbColumnType::Text => getter.try_get_db_value::<String>(value_type, DbValue::Text)?,
        DbColumnType::Varchar => getter.try_get_db_value::<String>(value_type, DbValue::Varchar)?,
        DbColumnType::Bpchar => getter.try_get_db_value::<String>(value_type, DbValue::Bpchar)?,
        DbColumnType::Json => {
            getter.try_get_db_value::<serde_json::Value>(value_type, DbValue::Json)?
        }
        DbColumnType::Jsonb => {
            getter.try_get_db_value::<serde_json::Value>(value_type, DbValue::Jsonb)?
        }
        DbColumnType::Jsonpath => {
            getter.try_get_db_value::<PgJsonPath>(value_type, |v| DbValue::Jsonpath(v.0))?
        }
        DbColumnType::Xml => getter.try_get_db_value::<PgXml>(value_type, |v| DbValue::Xml(v.0))?,
        DbColumnType::Timestamp => {
            getter.try_get_db_value::<chrono::NaiveDateTime>(value_type, DbValue::Timestamp)?
        }
        DbColumnType::Timestamptz => getter
            .try_get_db_value::<chrono::DateTime<chrono::Utc>>(value_type, DbValue::Timestamptz)?,
        DbColumnType::Date => {
            getter.try_get_db_value::<chrono::NaiveDate>(value_type, DbValue::Date)?
        }
        DbColumnType::Time => {
            getter.try_get_db_value::<chrono::NaiveTime>(value_type, DbValue::Time)?
        }
        DbColumnType::Timetz => getter
            .try_get_db_value::<PgTimeTz<chrono::NaiveTime, chrono::FixedOffset>>(
                value_type,
                |v| DbValue::Timetz(v.into()),
            )?,
        DbColumnType::Interval => {
            getter.try_get_db_value::<PgInterval>(value_type, |v| DbValue::Interval(v.into()))?
        }
        DbColumnType::Inet => getter.try_get_db_value::<IpAddr>(value_type, DbValue::Inet)?,
        DbColumnType::Cidr => getter.try_get_db_value::<IpAddr>(value_type, DbValue::Cidr)?,
        DbColumnType::Macaddr => {
            getter.try_get_db_value::<MacAddress>(value_type, DbValue::Macaddr)?
        }
        DbColumnType::Bit => getter.try_get_db_value::<BitVec>(value_type, DbValue::Bit)?,
        DbColumnType::Varbit => getter.try_get_db_value::<BitVec>(value_type, DbValue::Varbit)?,
        DbColumnType::Bytea => getter.try_get_db_value::<Vec<u8>>(value_type, DbValue::Bytea)?,
        DbColumnType::Tstzrange => getter
            .try_get_db_value::<PgRange<chrono::DateTime<chrono::Utc>>>(value_type, |v| {
                DbValue::Tstzrange(v.into())
            })?,
        DbColumnType::Tsrange => getter
            .try_get_db_value::<PgRange<chrono::NaiveDateTime>>(value_type, |v| {
                DbValue::Tsrange(v.into())
            })?,
        DbColumnType::Numrange => getter
            .try_get_db_value::<PgRange<BigDecimal>>(value_type, |v| DbValue::Numrange(v.into()))?,
        DbColumnType::Int4range => {
            getter.try_get_db_value::<PgRange<i32>>(value_type, |v| DbValue::Int4range(v.into()))?
        }
        DbColumnType::Int8range => {
            getter.try_get_db_value::<PgRange<i64>>(value_type, |v| DbValue::Int8range(v.into()))?
        }
        DbColumnType::Daterange => getter
            .try_get_db_value::<PgRange<chrono::NaiveDate>>(value_type, |v| {
                DbValue::Daterange(v.into())
            })?,
        DbColumnType::Money => {
            getter.try_get_db_value::<PgMoney>(value_type, |v| DbValue::Money(v.0))?
        }
        DbColumnType::Oid => getter.try_get_db_value::<Oid>(value_type, |v| DbValue::Oid(v.0))?,
        _ => Err(format!(
            "{} of {} is not supported",
            value_type, column_type
        ))?,
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
            PgTypeKind::Range(base_type) => {
                let base_type = get_db_column_type(base_type)?;
                Ok(DbColumnType::Range(RangeType::new(type_name, base_type)))
            }
            PgTypeKind::Array(element_type)
                if matches!(
                    element_type.kind(),
                    PgTypeKind::Enum(_)
                        | PgTypeKind::Domain(_)
                        | PgTypeKind::Composite(_)
                        | PgTypeKind::Range(_)
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

enum DbValueType {
    Primitive,
    Array,
    Range,
    RangeArray,
}

impl Display for DbValueType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbValueType::Primitive => write!(f, "Primitive"),
            DbValueType::Array => write!(f, "Array"),
            DbValueType::Range => write!(f, "Range"),
            DbValueType::RangeArray => write!(f, "Range Array"),
        }
    }
}

trait PgValueGetter {
    fn try_get_value<T>(&mut self) -> Result<T, String>
    where
        T: for<'a> sqlx::Decode<'a, sqlx::Postgres> + Type<sqlx::Postgres>;

    fn try_get_db_value<T>(
        &mut self,
        value_type: DbValueType,
        f: impl Fn(T) -> DbValue + Clone,
    ) -> Result<DbValue, String>
    where
        T: for<'a> sqlx::Decode<'a, sqlx::Postgres>
            + Type<sqlx::Postgres>
            + sqlx::postgres::PgHasArrayType,
    {
        match value_type {
            DbValueType::Primitive => {
                let v: Option<T> = self.try_get_value()?;
                Ok(DbValue::primitive_from_plain(v, f))
            }
            DbValueType::Array => {
                let v: Option<Vec<T>> = self.try_get_value()?;
                Ok(DbValue::array_from(v, f))
            }
            DbValueType::Range => {
                let v: Option<PgCustomRange<T>> = self.try_get_value()?;
                Ok(DbValue::primitive_from_plain(v, |v| {
                    get_range(v, f.clone())
                }))
            }
            DbValueType::RangeArray => {
                let v: Option<PgCustomRanges<T>> = self.try_get_value()?;
                Ok(DbValue::array_from(v.map(|v| v.0), |v| {
                    get_range(v, f.clone())
                }))
            }
        }
    }
}

trait PgValueSetter<'a> {
    fn try_set_value<T>(&mut self, value: T) -> Result<(), String>
    where
        T: 'a + sqlx::Encode<'a, sqlx::Postgres> + Type<sqlx::Postgres>;

    fn try_set_db_value<T>(
        &mut self,
        value: DbValue,
        value_type: DbValueType,
        f: impl Fn(DbValue) -> Option<T> + Clone,
    ) -> Result<(), String>
    where
        T: 'a
            + sqlx::Encode<'a, sqlx::Postgres>
            + Type<sqlx::Postgres>
            + sqlx::postgres::PgHasArrayType
            + Clone,
    {
        match value_type {
            DbValueType::Primitive => match value {
                DbValue::Array(_) | DbValue::Range(_) => {
                    Err(format!("{} do not support '{}' value", value_type, value))
                }
                _ => {
                    let v = f(value);
                    self.try_set_value(v)
                }
            },
            DbValueType::Array => {
                let vs: Vec<T> = get_array_plain_values(value, f.clone())?;
                if vs.is_empty() {
                    self.try_set_value(PgNull {})
                } else {
                    self.try_set_value(vs)
                }
            }
            DbValueType::Range => match value {
                DbValue::Range(v) => {
                    let v: PgCustomRange<T> = get_pg_range(v, f)?;
                    self.try_set_value(v)
                }
                _ => Err(format!("{} do not support '{}' value", value_type, value)),
            },
            DbValueType::RangeArray => {
                let ranges: Vec<_> = get_array_plain_values(value, |v| {
                    if let DbValue::Range(v) = v {
                        Some(v)
                    } else {
                        None
                    }
                })?;
                if ranges.is_empty() {
                    self.try_set_value(PgNull {})
                } else {
                    let vs: Vec<PgCustomRange<T>> = get_pg_ranges(ranges, f.clone())?;
                    self.try_set_value(PgCustomRanges(vs))
                }
            }
        }
    }
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

#[derive(Clone)]
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

#[derive(Clone)]
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

impl sqlx::postgres::PgHasArrayType for PgNull {
    fn array_type_info() -> sqlx::postgres::PgTypeInfo {
        // https://github.com/postgres/postgres/blob/master/src/include/catalog/pg_type.dat
        sqlx::postgres::PgTypeInfo::with_oid(Oid(2277)) // pseudo type array
    }
}

#[derive(Clone)]
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

struct PgCustomRange<T> {
    name: String,
    value: PgRange<T>,
}

impl<T> NamedType for PgCustomRange<T> {
    fn name(&self) -> String {
        self.name.clone()
    }
}

impl<T> PgCustomRange<T> {
    fn new(name: String, value: PgRange<T>) -> PgCustomRange<T> {
        Self { name, value }
    }
}

impl<T> sqlx::Type<sqlx::Postgres> for PgCustomRange<T> {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        sqlx::postgres::PgTypeInfo::with_oid(Oid(5080)) // pseudo type
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        matches!(ty.kind(), PgTypeKind::Range(_))
    }
}

impl<'q, T> sqlx::Encode<'q, sqlx::Postgres> for PgCustomRange<T>
where
    T: sqlx::Encode<'q, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <PgRange<T> as sqlx::Encode<sqlx::Postgres>>::encode_by_ref(&self.value, buf)
    }

    fn produces(&self) -> Option<sqlx::postgres::PgTypeInfo> {
        Some(sqlx::postgres::PgTypeInfo::with_name(
            self.name.clone().leak(),
        ))
    }
}

impl<'r, T> sqlx::Decode<'r, sqlx::Postgres> for PgCustomRange<T>
where
    T: for<'a> sqlx::Decode<'a, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    fn decode(
        value: sqlx::postgres::PgValueRef<'r>,
    ) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        let type_info = &value.type_info();
        let name = type_info.name().to_string();
        if let PgTypeKind::Range(_) = type_info.kind() {
            let v = <PgRange<T> as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
            Ok(PgCustomRange::new(name, v))
        } else {
            Err(format!("Type '{}' is not supported", name).into())
        }
    }
}

struct PgCustomRanges<T>(Vec<PgCustomRange<T>>);

impl<T> sqlx::Type<sqlx::Postgres> for PgCustomRanges<T> {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        sqlx::postgres::PgTypeInfo::with_oid(Oid(2277)) // pseudo type
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        matches!(ty.kind(), PgTypeKind::Array(ty) if <PgCustomRange<T> as sqlx::types::Type<sqlx::Postgres>>::compatible(ty))
    }
}

impl<'q, T> sqlx::Encode<'q, sqlx::Postgres> for PgCustomRanges<T>
where
    T: sqlx::Encode<'q, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <Vec<PgCustomRange<T>> as sqlx::Encode<sqlx::Postgres>>::encode_by_ref(&self.0, buf)
    }

    fn produces(&self) -> Option<sqlx::postgres::PgTypeInfo> {
        get_array_pg_type_info(&self.0)
    }
}

impl<'r, T> sqlx::Decode<'r, sqlx::Postgres> for PgCustomRanges<T>
where
    T: for<'a> sqlx::Decode<'a, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    fn decode(
        value: sqlx::postgres::PgValueRef<'r>,
    ) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        let value = <Vec<PgCustomRange<T>> as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
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
        PgTypeKind::Enum(_)
        | PgTypeKind::Composite(_)
        | PgTypeKind::Domain(_)
        | PgTypeKind::Range(_) => type_info.name().to_string(),
        PgTypeKind::Array(element_type)
            if matches!(
                element_type.kind(),
                PgTypeKind::Enum(_)
                    | PgTypeKind::Composite(_)
                    | PgTypeKind::Domain(_)
                    | PgTypeKind::Range(_)
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
