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

use async_trait::async_trait;
use bigdecimal::BigDecimal;
use itertools::Itertools;
use std::fmt::Display;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[async_trait]
pub trait DbResultSet {
    async fn get_columns(&self) -> Result<Vec<DbColumn>, Error>;

    async fn get_next(&self) -> Result<Option<Vec<DbRow>>, Error>;
}

#[derive(Clone, Debug)]
pub struct SimpleDbResultSet {
    columns: Vec<DbColumn>,
    rows: Arc<Mutex<Option<Vec<DbRow>>>>,
}

impl SimpleDbResultSet {
    pub fn new(columns: Vec<DbColumn>, rows: Option<Vec<DbRow>>) -> Self {
        Self {
            columns,
            rows: Arc::new(Mutex::new(rows)),
        }
    }
}

#[async_trait]
impl DbResultSet for SimpleDbResultSet {
    async fn get_columns(&self) -> Result<Vec<DbColumn>, Error> {
        Ok(self.columns.clone())
    }

    async fn get_next(&self) -> Result<Option<Vec<DbRow>>, Error> {
        let rows = self.rows.lock().unwrap().clone();
        if rows.is_some() {
            *self.rows.lock().unwrap() = None;
        }
        Ok(rows)
    }
}

#[derive(Clone, Debug, Default)]
pub struct EmptyDbResultSet {}

#[async_trait]
impl DbResultSet for EmptyDbResultSet {
    async fn get_columns(&self) -> Result<Vec<DbColumn>, Error> {
        Ok(vec![])
    }

    async fn get_next(&self) -> Result<Option<Vec<DbRow>>, Error> {
        Ok(None)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DbColumnTypePrimitive {
    Int8,
    Int16,
    Int32,
    Int64,
    Float,
    Double,
    Decimal,
    Boolean,
    Timestamp,
    Date,
    Time,
    Interval,
    Text,
    Blob,
    Json,
    Xml,
    Uuid,
}

impl Display for DbColumnTypePrimitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbColumnTypePrimitive::Int8 => write!(f, "int8"),
            DbColumnTypePrimitive::Int16 => write!(f, "int16"),
            DbColumnTypePrimitive::Int32 => write!(f, "int32"),
            DbColumnTypePrimitive::Int64 => write!(f, "int64"),
            DbColumnTypePrimitive::Float => write!(f, "float"),
            DbColumnTypePrimitive::Double => write!(f, "double"),
            DbColumnTypePrimitive::Decimal => write!(f, "decimal"),
            DbColumnTypePrimitive::Boolean => write!(f, "boolean"),
            DbColumnTypePrimitive::Timestamp => write!(f, "timestamp"),
            DbColumnTypePrimitive::Date => write!(f, "date"),
            DbColumnTypePrimitive::Time => write!(f, "time"),
            DbColumnTypePrimitive::Interval => write!(f, "interval"),
            DbColumnTypePrimitive::Text => write!(f, "text"),
            DbColumnTypePrimitive::Blob => write!(f, "blob"),
            DbColumnTypePrimitive::Json => write!(f, "json"),
            DbColumnTypePrimitive::Xml => write!(f, "xml"),
            DbColumnTypePrimitive::Uuid => write!(f, "uuid"),
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
    Int8(i8),
    Int16(i16),
    Int32(i32),
    Int64(i64),
    Float(f32),
    Double(f64),
    Decimal(BigDecimal),
    Boolean(bool),
    Timestamp(i64),
    Date(i64),
    Time(i64),
    Interval(i64),
    Text(String),
    Blob(Vec<u8>),
    Json(String),
    Xml(String),
    Uuid(Uuid),
    DbNull,
}

impl Display for DbValuePrimitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbValuePrimitive::Int8(v) => write!(f, "{}", v),
            DbValuePrimitive::Int16(v) => write!(f, "{}", v),
            DbValuePrimitive::Int32(v) => write!(f, "{}", v),
            DbValuePrimitive::Int64(v) => write!(f, "{}", v),
            DbValuePrimitive::Float(v) => write!(f, "{}", v),
            DbValuePrimitive::Double(v) => write!(f, "{}", v),
            DbValuePrimitive::Decimal(v) => write!(f, "{}", v),
            DbValuePrimitive::Boolean(v) => write!(f, "{}", v),
            DbValuePrimitive::Timestamp(v) => write!(f, "{}", v),
            DbValuePrimitive::Date(v) => write!(f, "{}", v),
            DbValuePrimitive::Time(v) => write!(f, "{}", v),
            DbValuePrimitive::Interval(v) => write!(f, "{}", v),
            DbValuePrimitive::Text(v) => write!(f, "{}", v),
            DbValuePrimitive::Blob(v) => write!(f, "{:?}", v),
            DbValuePrimitive::Json(v) => write!(f, "{}", v),
            DbValuePrimitive::Xml(v) => write!(f, "{}", v),
            DbValuePrimitive::Uuid(v) => write!(f, "{}", v),
            DbValuePrimitive::DbNull => write!(f, "NULL"),
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

#[derive(Clone, Debug, PartialEq)]
pub struct DbRow {
    pub values: Vec<DbValue>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DbColumn {
    pub ordinal: u64,
    pub name: String,
    pub db_type: DbColumnType,
    pub db_type_name: String,
}

// #[derive(Clone, Debug)]
// pub struct DbColumnTypeMeta {
//     pub name: String,
//     pub db_type: DbColumnType,
//     pub db_type_flags: HashSet<DbColumnTypeFlag>,
//     pub foreign_key: Option<String>,
// }
//
// #[derive(Clone, Debug)]
// pub enum DbColumnTypeFlag {
//     PrimaryKey,
//     ForeignKey,
//     Unique,
//     Nullable,
//     Generated,
//     AutoIncrement,
//     DefaultValue,
//     Indexed,
// }

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    ConnectionFailure(String),
    QueryParameterFailure(String),
    QueryExecutionFailure(String),
    QueryResponseFailure(String),
    Other(String),
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

pub(crate) fn get_plain_values<T>(
    values: Vec<DbValuePrimitive>,
    f: impl Fn(DbValuePrimitive) -> Option<T>,
) -> Result<Vec<T>, String> {
    let mut result: Vec<T> = Vec::new();
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
