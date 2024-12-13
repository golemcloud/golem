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
    Year(u16),
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
