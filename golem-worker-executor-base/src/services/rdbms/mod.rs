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

pub(crate) mod metrics;
pub mod mysql;
pub mod postgres;
pub(crate) mod sqlx_common;

#[cfg(test)]
mod tests;

use crate::error::GolemError;
use crate::services::golem_config::RdbmsConfig;
use crate::services::rdbms::mysql::MysqlType;
use crate::services::rdbms::postgres::PostgresType;
use async_trait::async_trait;
use bincode::{BorrowDecode, Decode, Encode};
use chrono::{Datelike, Offset, Timelike};
use golem_common::model::WorkerId;
use golem_wasm_ast::analysis::{analysed_type, AnalysedType};
use golem_wasm_rpc::{IntoValue, Value, ValueAndType};
use itertools::Itertools;
use mac_address::MacAddress;
use std::collections::{Bound, HashMap, HashSet};
use std::fmt::{Debug, Display};
use std::net::IpAddr;
use std::sync::Arc;
use url::Url;

pub trait RdbmsType: Debug + Display + Default + Send {
    type DbColumn: Clone
        + Send
        + Sync
        + PartialEq
        + Debug
        + Decode
        + for<'de> BorrowDecode<'de>
        + Encode
        + RdbmsIntoValueAndType
        + 'static;
    type DbValue: Clone
        + Send
        + Sync
        + PartialEq
        + Debug
        + Decode
        + for<'de> BorrowDecode<'de>
        + Encode
        + RdbmsIntoValueAndType
        + 'static;
}

#[derive(Clone)]
pub struct RdbmsStatus {
    pools: HashMap<RdbmsPoolKey, HashSet<WorkerId>>,
}

impl Display for RdbmsStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (key, workers) in self.pools.iter() {
            writeln!(f, "{}: {}", key, workers.iter().join(", "))?;
        }

        Ok(())
    }
}

#[async_trait]
pub trait DbTransaction<T: RdbmsType> {
    async fn execute(&self, statement: &str, params: Vec<T::DbValue>) -> Result<u64, Error>
    where
        <T as RdbmsType>::DbValue: 'async_trait;

    async fn query(&self, statement: &str, params: Vec<T::DbValue>) -> Result<DbResult<T>, Error>
    where
        <T as RdbmsType>::DbValue: 'async_trait;

    async fn query_stream(
        &self,
        statement: &str,
        params: Vec<T::DbValue>,
    ) -> Result<Arc<dyn DbResultStream<T> + Send + Sync>, Error>
    where
        <T as RdbmsType>::DbValue: 'async_trait;

    async fn commit(&self) -> Result<(), Error>;

    async fn rollback(&self) -> Result<(), Error>;

    async fn rollback_if_open(&self) -> Result<(), Error>;
}

#[async_trait]
pub trait Rdbms<T: RdbmsType> {
    async fn create(&self, address: &str, worker_id: &WorkerId) -> Result<RdbmsPoolKey, Error>;

    fn exists(&self, key: &RdbmsPoolKey, worker_id: &WorkerId) -> bool;

    fn remove(&self, key: &RdbmsPoolKey, worker_id: &WorkerId) -> bool;

    async fn execute(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
        statement: &str,
        params: Vec<T::DbValue>,
    ) -> Result<u64, Error>
    where
        <T as RdbmsType>::DbValue: 'async_trait;

    async fn query_stream(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
        statement: &str,
        params: Vec<T::DbValue>,
    ) -> Result<Arc<dyn DbResultStream<T> + Send + Sync>, Error>
    where
        <T as RdbmsType>::DbValue: 'async_trait;

    async fn query(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
        statement: &str,
        params: Vec<T::DbValue>,
    ) -> Result<DbResult<T>, Error>
    where
        <T as RdbmsType>::DbValue: 'async_trait;

    async fn begin_transaction(
        &self,
        key: &RdbmsPoolKey,
        worker_id: &WorkerId,
    ) -> Result<Arc<dyn DbTransaction<T> + Send + Sync>, Error>;

    fn status(&self) -> RdbmsStatus;
}

pub trait RdbmsService {
    fn mysql(&self) -> Arc<dyn Rdbms<MysqlType> + Send + Sync>;
    fn postgres(&self) -> Arc<dyn Rdbms<PostgresType> + Send + Sync>;
}

pub trait RdbmsTypeService<T: RdbmsType> {
    fn rdbms_type_service(&self) -> Arc<dyn Rdbms<T> + Send + Sync>;
}

impl RdbmsTypeService<MysqlType> for dyn RdbmsService + Send + Sync {
    fn rdbms_type_service(&self) -> Arc<dyn Rdbms<MysqlType> + Send + Sync> {
        self.mysql()
    }
}

impl RdbmsTypeService<PostgresType> for dyn RdbmsService + Send + Sync {
    fn rdbms_type_service(&self) -> Arc<dyn Rdbms<PostgresType> + Send + Sync> {
        self.postgres()
    }
}

#[derive(Clone)]
pub struct RdbmsServiceDefault {
    mysql: Arc<dyn Rdbms<MysqlType> + Send + Sync>,
    postgres: Arc<dyn Rdbms<PostgresType> + Send + Sync>,
}

impl RdbmsServiceDefault {
    pub fn new(config: RdbmsConfig) -> Self {
        Self {
            mysql: MysqlType::new_rdbms(config),
            postgres: PostgresType::new_rdbms(config),
        }
    }
}

impl Default for RdbmsServiceDefault {
    fn default() -> Self {
        Self::new(RdbmsConfig::default())
    }
}

impl RdbmsService for RdbmsServiceDefault {
    fn mysql(&self) -> Arc<dyn Rdbms<MysqlType> + Send + Sync> {
        self.mysql.clone()
    }

    fn postgres(&self) -> Arc<dyn Rdbms<PostgresType> + Send + Sync> {
        self.postgres.clone()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Encode, Decode)]
pub struct RdbmsPoolKey {
    #[bincode(with_serde)]
    pub address: Url,
}

impl RdbmsPoolKey {
    pub fn new(address: Url) -> Self {
        Self { address }
    }

    pub fn from(address: &str) -> Result<Self, String> {
        let url = Url::parse(address).map_err(|e| e.to_string())?;
        Ok(Self::new(url))
    }

    pub fn masked_address(&self) -> String {
        let mut output: String = self.address.scheme().to_string();
        output.push_str("://");

        let username = self.address.username();
        output.push_str(username);

        let password = self.address.password();
        if password.is_some() {
            output.push_str(":*****");
        }

        if let Some(h) = self.address.host_str() {
            if !username.is_empty() || password.is_some() {
                output.push('@');
            }

            output.push_str(h);

            if let Some(p) = self.address.port() {
                output.push(':');
                output.push_str(p.to_string().as_str());
            }
        }

        output.push_str(self.address.path());

        let query_pairs = self.address.query_pairs();

        if query_pairs.count() > 0 {
            output.push('?');
        }
        for (index, (key, value)) in query_pairs.enumerate() {
            let key = &*key;
            output.push_str(key);
            output.push('=');

            if key == "password" || key == "secret" {
                output.push_str("*****");
            } else {
                output.push_str(&value);
            }
            if index < query_pairs.count() - 1 {
                output.push('&');
            }
        }

        output
    }
}

impl Display for RdbmsPoolKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.masked_address())
    }
}

impl IntoValue for RdbmsPoolKey {
    fn into_value(self) -> Value {
        Value::Record(vec![self.address.to_string().into_value()])
    }

    fn get_type() -> AnalysedType {
        analysed_type::record(vec![analysed_type::field("address", analysed_type::str())])
    }
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct DbRow<T: 'static> {
    pub values: Vec<T>,
}

impl<T> RdbmsIntoValueAndType for DbRow<T>
where
    Vec<T>: RdbmsIntoValueAndType,
{
    fn into_value_and_type(self) -> ValueAndType {
        let v = RdbmsIntoValueAndType::into_value_and_type(self.values);
        let t = analysed_type::record(vec![analysed_type::field("values", v.typ)]);
        ValueAndType::new(Value::Record(vec![v.value]), t)
    }

    fn get_base_type() -> AnalysedType {
        analysed_type::record(vec![analysed_type::field(
            "values",
            <Vec<T>>::get_base_type(),
        )])
    }
}

impl<T> AnalysedTypeMerger for DbRow<T>
where
    T: AnalysedTypeMerger,
    Vec<T>: RdbmsIntoValueAndType,
{
    fn merge_types(first: AnalysedType, second: AnalysedType) -> AnalysedType {
        if let (AnalysedType::Record(f), AnalysedType::Record(s)) = (first.clone(), second) {
            if f.fields.len() == s.fields.len() {
                let mut fields = Vec::with_capacity(f.fields.len());
                let mut ok = true;

                for (fc, sc) in f.fields.into_iter().zip(s.fields.into_iter()) {
                    if fc.name != sc.name {
                        ok = false;
                        break;
                    }
                    if fc.name == "values" {
                        let t = <Vec<T>>::merge_types(fc.typ, sc.typ);
                        fields.push(analysed_type::field(fc.name.as_str(), t));
                    } else {
                        fields.push(fc);
                    }
                }
                if ok {
                    analysed_type::record(fields)
                } else {
                    first
                }
            } else {
                first
            }
        } else {
            first
        }
    }
}

#[async_trait]
pub trait DbResultStream<T: RdbmsType> {
    async fn get_columns(&self) -> Result<Vec<T::DbColumn>, Error>;

    async fn get_next(&self) -> Result<Option<Vec<DbRow<T::DbValue>>>, Error>;
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct DbResult<T: RdbmsType + 'static> {
    pub columns: Vec<T::DbColumn>,
    pub rows: Vec<DbRow<T::DbValue>>,
}

impl<T: RdbmsType> DbResult<T> {
    pub fn new(columns: Vec<T::DbColumn>, rows: Vec<DbRow<T::DbValue>>) -> Self {
        Self { columns, rows }
    }

    pub fn empty() -> Self {
        Self::new(vec![], vec![])
    }

    #[allow(dead_code)]
    pub(crate) async fn from(
        result_set: Arc<dyn DbResultStream<T> + Send + Sync>,
    ) -> Result<DbResult<T>, Error> {
        let columns = result_set.get_columns().await?;
        let mut rows: Vec<DbRow<T::DbValue>> = vec![];

        while let Some(vs) = result_set.get_next().await? {
            rows.extend(vs);
        }
        Ok(DbResult::new(columns, rows))
    }
}

impl<T> RdbmsIntoValueAndType for DbResult<T>
where
    T: RdbmsType,
    Vec<T::DbColumn>: RdbmsIntoValueAndType,
    Vec<DbRow<T::DbValue>>: RdbmsIntoValueAndType,
{
    fn into_value_and_type(self) -> ValueAndType {
        let cs = RdbmsIntoValueAndType::into_value_and_type(self.columns);
        let rs = RdbmsIntoValueAndType::into_value_and_type(self.rows);
        let t = analysed_type::record(vec![
            analysed_type::field("columns", cs.typ),
            analysed_type::field("rows", rs.typ),
        ]);
        ValueAndType::new(Value::Record(vec![cs.value, rs.value]), t)
    }

    fn get_base_type() -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("columns", <Vec<T::DbColumn>>::get_base_type()),
            analysed_type::field("rows", <Vec<DbRow<T::DbValue>>>::get_base_type()),
        ])
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Encode, Decode)]
pub enum Error {
    ConnectionFailure(String),
    QueryParameterFailure(String),
    QueryExecutionFailure(String),
    QueryResponseFailure(String),
    Other(String),
}

impl Error {
    pub(crate) fn connection_failure<E: Display>(error: E) -> Error {
        Self::ConnectionFailure(error.to_string())
    }

    pub(crate) fn query_execution_failure<E: Display>(error: E) -> Error {
        Self::QueryExecutionFailure(error.to_string())
    }

    pub(crate) fn query_response_failure<E: Display>(error: E) -> Error {
        Self::QueryResponseFailure(error.to_string())
    }

    pub(crate) fn other_response_failure<E: Display>(error: E) -> Error {
        Self::Other(error.to_string())
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::ConnectionFailure(msg) => write!(f, "ConnectionFailure: {}", msg),
            Error::QueryParameterFailure(msg) => write!(f, "QueryParameterFailure: {}", msg),
            Error::QueryExecutionFailure(msg) => write!(f, "QueryExecutionFailure: {}", msg),
            Error::QueryResponseFailure(msg) => write!(f, "QueryResponseFailure: {}", msg),
            Error::Other(msg) => write!(f, "Other: {}", msg),
        }
    }
}

impl IntoValue for Error {
    fn into_value(self) -> Value {
        match self {
            Error::ConnectionFailure(errors) => Value::Variant {
                case_idx: 0,
                case_value: Some(Box::new(errors.into_value())),
            },
            Error::QueryParameterFailure(error) => Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(error.into_value())),
            },
            Error::QueryExecutionFailure(error) => Value::Variant {
                case_idx: 2,
                case_value: Some(Box::new(error.into_value())),
            },
            Error::QueryResponseFailure(error) => Value::Variant {
                case_idx: 3,
                case_value: Some(Box::new(error.into_value())),
            },
            Error::Other(error) => Value::Variant {
                case_idx: 4,
                case_value: Some(Box::new(error.into_value())),
            },
        }
    }

    fn get_type() -> AnalysedType {
        analysed_type::variant(vec![
            analysed_type::case("ConnectionFailure", analysed_type::str()),
            analysed_type::case("QueryParameterFailure", analysed_type::str()),
            analysed_type::case("QueryExecutionFailure", analysed_type::str()),
            analysed_type::case("QueryResponseFailure", analysed_type::str()),
            analysed_type::case("Other", analysed_type::str()),
        ])
    }
}

impl From<GolemError> for Error {
    fn from(value: GolemError) -> Self {
        Self::other_response_failure(value)
    }
}

pub trait AnalysedTypeMerger {
    fn merge_types(first: AnalysedType, second: AnalysedType) -> AnalysedType;

    fn merge_types_opt(
        first: Option<AnalysedType>,
        second: Option<AnalysedType>,
    ) -> Option<AnalysedType> {
        match (first, second) {
            (Some(f), Some(s)) => Some(Self::merge_types(f, s)),
            (None, Some(s)) => Some(s),
            (Some(f), None) => Some(f),
            _ => None,
        }
    }
}

impl<T: AnalysedTypeMerger> AnalysedTypeMerger for Vec<T> {
    fn merge_types(first: AnalysedType, second: AnalysedType) -> AnalysedType {
        if let (AnalysedType::List(f), AnalysedType::List(s)) = (first.clone(), second) {
            let t = T::merge_types(*f.inner, *s.inner);
            analysed_type::list(t)
        } else {
            first
        }
    }
}

pub trait RdbmsIntoValueAndType {
    fn into_value_and_type(self) -> ValueAndType;

    fn get_base_type() -> AnalysedType;
}

impl<T: RdbmsIntoValueAndType> RdbmsIntoValueAndType for Option<T> {
    fn into_value_and_type(self) -> ValueAndType {
        match self {
            Some(v) => {
                let v = v.into_value_and_type();
                ValueAndType::new(
                    Value::Option(Some(Box::new(v.value))),
                    analysed_type::option(v.typ),
                )
            }
            None => ValueAndType::new(Value::Option(None), Self::get_base_type()),
        }
    }

    fn get_base_type() -> AnalysedType {
        analysed_type::option(T::get_base_type())
    }
}

impl<S: RdbmsIntoValueAndType, E: IntoValue> RdbmsIntoValueAndType for Result<S, E> {
    fn into_value_and_type(self) -> ValueAndType {
        match self {
            Ok(v) => {
                let v = v.into_value_and_type();
                ValueAndType::new(
                    Value::Result(Ok(Some(Box::new(v.value)))),
                    analysed_type::result(v.typ, E::get_type()),
                )
            }
            Err(e) => ValueAndType::new(
                Value::Result(Err(Some(Box::new(e.into_value())))),
                Self::get_base_type(),
            ),
        }
    }

    fn get_base_type() -> AnalysedType {
        analysed_type::result(S::get_base_type(), E::get_type())
    }
}

impl<T: RdbmsIntoValueAndType + AnalysedTypeMerger> RdbmsIntoValueAndType for Vec<T> {
    fn into_value_and_type(self) -> ValueAndType {
        let mut vs = Vec::with_capacity(self.len());
        let mut t: Option<AnalysedType> = None;
        for v in self {
            let v = v.into_value_and_type();
            t = match t {
                None => Some(v.typ),
                Some(t) => Some(T::merge_types(t, v.typ)),
            };
            vs.push(v.value);
        }

        let t = t.unwrap_or(T::get_base_type());
        ValueAndType::new(Value::List(vs), analysed_type::list(t))
    }

    fn get_base_type() -> AnalysedType {
        analysed_type::list(T::get_base_type())
    }
}

impl RdbmsIntoValueAndType for chrono::NaiveDate {
    fn into_value_and_type(self) -> ValueAndType {
        let year = self.year();
        let month = self.month() as u8;
        let day = self.day() as u8;
        ValueAndType::new(
            Value::Record(vec![Value::S32(year), Value::U8(month), Value::U8(day)]),
            Self::get_base_type(),
        )
    }

    fn get_base_type() -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("year", analysed_type::s32()),
            analysed_type::field("month", analysed_type::u8()),
            analysed_type::field("day", analysed_type::u8()),
        ])
    }
}

impl RdbmsIntoValueAndType for chrono::NaiveTime {
    fn into_value_and_type(self) -> ValueAndType {
        let hour = self.hour() as u8;
        let minute = self.minute() as u8;
        let second = self.second() as u8;
        let nanosecond = self.nanosecond();
        ValueAndType::new(
            Value::Record(vec![
                Value::U8(hour),
                Value::U8(minute),
                Value::U8(second),
                Value::U32(nanosecond),
            ]),
            Self::get_base_type(),
        )
    }

    fn get_base_type() -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("hours", analysed_type::u8()),
            analysed_type::field("minutes", analysed_type::u8()),
            analysed_type::field("seconds", analysed_type::u8()),
            analysed_type::field("nanoseconds", analysed_type::u32()),
        ])
    }
}

impl RdbmsIntoValueAndType for chrono::NaiveDateTime {
    fn into_value_and_type(self) -> ValueAndType {
        let date = self.date().into_value_and_type();
        let time = self.time().into_value_and_type();
        ValueAndType::new(
            Value::Record(vec![date.value, time.value]),
            Self::get_base_type(),
        )
    }

    fn get_base_type() -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("date", chrono::NaiveDate::get_base_type()),
            analysed_type::field("time", chrono::NaiveTime::get_base_type()),
        ])
    }
}

impl RdbmsIntoValueAndType for chrono::DateTime<chrono::Utc> {
    fn into_value_and_type(self) -> ValueAndType {
        let timestamp = self.naive_utc().into_value_and_type();
        let offset = self.offset().fix().local_minus_utc();
        ValueAndType::new(
            Value::Record(vec![timestamp.value, Value::S32(offset)]),
            Self::get_base_type(),
        )
    }

    fn get_base_type() -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("timestamp", chrono::NaiveDateTime::get_base_type()),
            analysed_type::field("offset", analysed_type::s32()),
        ])
    }
}

impl RdbmsIntoValueAndType for MacAddress {
    fn into_value_and_type(self) -> ValueAndType {
        let vs = self.bytes().into_iter().map(Value::U8).collect();
        ValueAndType::new(Value::Record(vec![Value::Tuple(vs)]), Self::get_base_type())
    }

    fn get_base_type() -> AnalysedType {
        analysed_type::record(vec![analysed_type::field(
            "octets",
            analysed_type::tuple(vec![analysed_type::u8(); 6]),
        )])
    }
}

impl RdbmsIntoValueAndType for IpAddr {
    fn into_value_and_type(self) -> ValueAndType {
        let v = match self {
            IpAddr::V4(v) => {
                let vs = v.octets().into_iter().map(Value::U8).collect();
                Value::Variant {
                    case_idx: 0,
                    case_value: Some(Box::new(Value::Tuple(vs))),
                }
            }
            IpAddr::V6(v) => {
                let vs = v.segments().into_iter().map(Value::U16).collect();
                Value::Variant {
                    case_idx: 1,
                    case_value: Some(Box::new(Value::Tuple(vs))),
                }
            }
        };

        ValueAndType::new(v, Self::get_base_type())
    }

    fn get_base_type() -> AnalysedType {
        analysed_type::variant(vec![
            analysed_type::case("ipv4", analysed_type::tuple(vec![analysed_type::u8(); 4])),
            analysed_type::case("ipv6", analysed_type::tuple(vec![analysed_type::u16(); 8])),
        ])
    }
}

impl<T: RdbmsIntoValueAndType> RdbmsIntoValueAndType for Bound<T> {
    fn into_value_and_type(self) -> ValueAndType {
        let (v, t) = get_bound_value(self);
        let t = t.unwrap_or(Self::get_base_type());
        ValueAndType::new(v, t)
    }

    fn get_base_type() -> AnalysedType {
        get_bound_analysed_type(T::get_base_type())
    }
}

fn get_bound_value<T: RdbmsIntoValueAndType>(value: Bound<T>) -> (Value, Option<AnalysedType>) {
    match value {
        Bound::Included(t) => {
            let v = t.into_value_and_type();
            let value = Value::Variant {
                case_idx: 0,
                case_value: Some(Box::new(v.value)),
            };
            (value, Some(v.typ))
        }
        Bound::Excluded(t) => {
            let v = t.into_value_and_type();
            let value = Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(v.value)),
            };
            (value, Some(v.typ))
        }
        Bound::Unbounded => {
            let value = Value::Variant {
                case_idx: 2,
                case_value: None,
            };
            (value, None)
        }
    }
}

fn get_bound_analysed_type(base_type: AnalysedType) -> AnalysedType {
    analysed_type::variant(vec![
        analysed_type::case("included", base_type.clone()),
        analysed_type::case("excluded", base_type.clone()),
        analysed_type::unit_case("unbounded"),
    ])
}
