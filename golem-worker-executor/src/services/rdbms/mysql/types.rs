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

use bigdecimal::BigDecimal;
use bit_vec::BitVec;
use golem_common::model::oplog::payload::types::SerializableDbValue;
use golem_common::model::oplog::types::{
    EnumerationType, SerializableDbColumn, SerializableDbColumnType, SerializableDbColumnTypeNode,
    SerializableDbValueNode,
};
use golem_wasm::NodeIndex;
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

impl From<DbColumnType> for SerializableDbColumnType {
    fn from(value: DbColumnType) -> Self {
        let mut result = SerializableDbColumnType { nodes: vec![] };
        match value {
            DbColumnType::Boolean => result.nodes.push(SerializableDbColumnTypeNode::Boolean),
            DbColumnType::Tinyint => result.nodes.push(SerializableDbColumnTypeNode::Tinyint),
            DbColumnType::Smallint => result.nodes.push(SerializableDbColumnTypeNode::Smallint),
            DbColumnType::Mediumint => result.nodes.push(SerializableDbColumnTypeNode::Int),
            DbColumnType::Int => result.nodes.push(SerializableDbColumnTypeNode::Int),
            DbColumnType::Bigint => result.nodes.push(SerializableDbColumnTypeNode::Bigint),
            DbColumnType::TinyintUnsigned => result
                .nodes
                .push(SerializableDbColumnTypeNode::TinyintUnsigned),
            DbColumnType::SmallintUnsigned => result
                .nodes
                .push(SerializableDbColumnTypeNode::SmallintUnsigned),
            DbColumnType::MediumintUnsigned => result
                .nodes
                .push(SerializableDbColumnTypeNode::MediumintUnsigned),
            DbColumnType::IntUnsigned => {
                result.nodes.push(SerializableDbColumnTypeNode::IntUnsigned)
            }
            DbColumnType::BigintUnsigned => result
                .nodes
                .push(SerializableDbColumnTypeNode::BigintUnsigned),
            DbColumnType::Float => result.nodes.push(SerializableDbColumnTypeNode::Float),
            DbColumnType::Double => result.nodes.push(SerializableDbColumnTypeNode::Double),
            DbColumnType::Decimal => result.nodes.push(SerializableDbColumnTypeNode::Decimal),
            DbColumnType::Date => result.nodes.push(SerializableDbColumnTypeNode::Date),
            DbColumnType::Datetime => result.nodes.push(SerializableDbColumnTypeNode::Timestamptz),
            DbColumnType::Timestamp => result.nodes.push(SerializableDbColumnTypeNode::Timestamptz),
            DbColumnType::Time => result.nodes.push(SerializableDbColumnTypeNode::Time),
            DbColumnType::Year => result.nodes.push(SerializableDbColumnTypeNode::Year),
            DbColumnType::Fixchar => result.nodes.push(SerializableDbColumnTypeNode::Fixchar),
            DbColumnType::Varchar => result.nodes.push(SerializableDbColumnTypeNode::Varchar),
            DbColumnType::Tinytext => result.nodes.push(SerializableDbColumnTypeNode::Tinytext),
            DbColumnType::Text => result.nodes.push(SerializableDbColumnTypeNode::Text),
            DbColumnType::Mediumtext => result.nodes.push(SerializableDbColumnTypeNode::Mediumtext),
            DbColumnType::Longtext => result.nodes.push(SerializableDbColumnTypeNode::Longtext),
            DbColumnType::Binary => result.nodes.push(SerializableDbColumnTypeNode::Binary),
            DbColumnType::Varbinary => result.nodes.push(SerializableDbColumnTypeNode::Varbinary),
            DbColumnType::Tinyblob => result.nodes.push(SerializableDbColumnTypeNode::Tinyblob),
            DbColumnType::Blob => result.nodes.push(SerializableDbColumnTypeNode::Blob),
            DbColumnType::Mediumblob => result.nodes.push(SerializableDbColumnTypeNode::Mediumblob),
            DbColumnType::Longblob => result.nodes.push(SerializableDbColumnTypeNode::Longblob),
            DbColumnType::Enumeration => result.nodes.push(
                SerializableDbColumnTypeNode::Enumeration(EnumerationType::new("".to_string())),
            ),
            DbColumnType::Set => result.nodes.push(SerializableDbColumnTypeNode::Set),
            DbColumnType::Bit => result.nodes.push(SerializableDbColumnTypeNode::Bit),
            DbColumnType::Json => result.nodes.push(SerializableDbColumnTypeNode::Json),
        }
        result
    }
}

impl TryFrom<SerializableDbColumnType> for DbColumnType {
    type Error = String;

    fn try_from(mut value: SerializableDbColumnType) -> Result<Self, Self::Error> {
        if value.nodes.len() != 1 {
            return Err("SerializableDbColumnType must have exactly one node".to_string());
        }
        let node = value.nodes.remove(0);
        match node {
            SerializableDbColumnTypeNode::Boolean => Ok(DbColumnType::Boolean),
            SerializableDbColumnTypeNode::Tinyint => Ok(DbColumnType::Tinyint),
            SerializableDbColumnTypeNode::Smallint => Ok(DbColumnType::Smallint),
            SerializableDbColumnTypeNode::Int => Ok(DbColumnType::Int),
            SerializableDbColumnTypeNode::Bigint => Ok(DbColumnType::Bigint),
            SerializableDbColumnTypeNode::TinyintUnsigned => Ok(DbColumnType::TinyintUnsigned),
            SerializableDbColumnTypeNode::SmallintUnsigned => Ok(DbColumnType::SmallintUnsigned),
            SerializableDbColumnTypeNode::MediumintUnsigned => Ok(DbColumnType::MediumintUnsigned),
            SerializableDbColumnTypeNode::IntUnsigned => Ok(DbColumnType::IntUnsigned),
            SerializableDbColumnTypeNode::BigintUnsigned => Ok(DbColumnType::BigintUnsigned),
            SerializableDbColumnTypeNode::Float => Ok(DbColumnType::Float),
            SerializableDbColumnTypeNode::Double => Ok(DbColumnType::Double),
            SerializableDbColumnTypeNode::Decimal => Ok(DbColumnType::Decimal),
            SerializableDbColumnTypeNode::Date => Ok(DbColumnType::Date),
            SerializableDbColumnTypeNode::Timestamptz => Ok(DbColumnType::Timestamp),
            SerializableDbColumnTypeNode::Time => Ok(DbColumnType::Time),
            SerializableDbColumnTypeNode::Year => Ok(DbColumnType::Year),
            SerializableDbColumnTypeNode::Fixchar => Ok(DbColumnType::Fixchar),
            SerializableDbColumnTypeNode::Varchar => Ok(DbColumnType::Varchar),
            SerializableDbColumnTypeNode::Tinytext => Ok(DbColumnType::Tinytext),
            SerializableDbColumnTypeNode::Text => Ok(DbColumnType::Text),
            SerializableDbColumnTypeNode::Mediumtext => Ok(DbColumnType::Mediumtext),
            SerializableDbColumnTypeNode::Longtext => Ok(DbColumnType::Longtext),
            SerializableDbColumnTypeNode::Binary => Ok(DbColumnType::Binary),
            SerializableDbColumnTypeNode::Varbinary => Ok(DbColumnType::Varbinary),
            SerializableDbColumnTypeNode::Tinyblob => Ok(DbColumnType::Tinyblob),
            SerializableDbColumnTypeNode::Blob => Ok(DbColumnType::Blob),
            SerializableDbColumnTypeNode::Mediumblob => Ok(DbColumnType::Mediumblob),
            SerializableDbColumnTypeNode::Longblob => Ok(DbColumnType::Longblob),
            SerializableDbColumnTypeNode::Enumeration(_) => Ok(DbColumnType::Enumeration),
            SerializableDbColumnTypeNode::Set => Ok(DbColumnType::Set),
            SerializableDbColumnTypeNode::Bit => Ok(DbColumnType::Bit),
            SerializableDbColumnTypeNode::Json => Ok(DbColumnType::Json),
            _ => Err(format!(
                "Unsupported SerializableDbColumnTypeNode variant for MySQL: {:?}",
                node
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DbValue {
    Boolean(bool),
    Tinyint(i8),
    Smallint(i16),
    Mediumint(i32),
    Int(i32),
    Bigint(i64),
    TinyintUnsigned(u8),
    SmallintUnsigned(u16),
    MediumintUnsigned(u32),
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
    Json(String),
    Null,
}

impl Display for DbValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbValue::Boolean(v) => write!(f, "{v}"),
            DbValue::Tinyint(v) => write!(f, "{v}"),
            DbValue::Smallint(v) => write!(f, "{v}"),
            DbValue::Mediumint(v) => write!(f, "{v}"),
            DbValue::Int(v) => write!(f, "{v}"),
            DbValue::Bigint(v) => write!(f, "{v}"),
            DbValue::TinyintUnsigned(v) => write!(f, "{v}"),
            DbValue::SmallintUnsigned(v) => write!(f, "{v}"),
            DbValue::MediumintUnsigned(v) => write!(f, "{v}"),
            DbValue::IntUnsigned(v) => write!(f, "{v}"),
            DbValue::BigintUnsigned(v) => write!(f, "{v}"),
            DbValue::Float(v) => write!(f, "{v}"),
            DbValue::Double(v) => write!(f, "{v}"),
            DbValue::Decimal(v) => write!(f, "{v}"),
            DbValue::Date(v) => write!(f, "{v}"),
            DbValue::Datetime(v) => write!(f, "{v}"),
            DbValue::Timestamp(v) => write!(f, "{v}"),
            DbValue::Time(v) => write!(f, "{v}"),
            DbValue::Year(v) => write!(f, "{v}"),
            DbValue::Fixchar(v) => write!(f, "{v}"),
            DbValue::Varchar(v) => write!(f, "{v}"),
            DbValue::Tinytext(v) => write!(f, "{v}"),
            DbValue::Text(v) => write!(f, "{v}"),
            DbValue::Mediumtext(v) => write!(f, "{v}"),
            DbValue::Longtext(v) => write!(f, "{v}"),
            DbValue::Binary(v) => write!(f, "{v:?}"),
            DbValue::Varbinary(v) => write!(f, "{v:?}"),
            DbValue::Tinyblob(v) => write!(f, "{v:?}"),
            DbValue::Blob(v) => write!(f, "{v:?}"),
            DbValue::Mediumblob(v) => write!(f, "{v:?}"),
            DbValue::Longblob(v) => write!(f, "{v:?}"),
            DbValue::Enumeration(v) => write!(f, "{v}"),
            DbValue::Set(v) => write!(f, "{v}"),
            DbValue::Bit(v) => write!(f, "{v:?}"),
            DbValue::Json(v) => write!(f, "{v}"),
            DbValue::Null => write!(f, "NULL"),
        }
    }
}

impl From<DbValue> for SerializableDbValue {
    fn from(value: DbValue) -> Self {
        fn add_node(target: &mut SerializableDbValue, value: SerializableDbValueNode) -> NodeIndex {
            target.nodes.push(value);
            (target.nodes.len() - 1) as NodeIndex
        }

        fn add_db_value(target: &mut SerializableDbValue, value: DbValue) -> NodeIndex {
            match value {
                DbValue::Boolean(v) => add_node(target, SerializableDbValueNode::Boolean(v)),
                DbValue::Tinyint(v) => add_node(target, SerializableDbValueNode::Tinyint(v)),
                DbValue::Smallint(v) => add_node(target, SerializableDbValueNode::Smallint(v)),
                DbValue::Mediumint(v) => add_node(target, SerializableDbValueNode::Mediumint(v)),
                DbValue::Int(v) => add_node(target, SerializableDbValueNode::Int(v)),
                DbValue::Bigint(v) => add_node(target, SerializableDbValueNode::Bigint(v)),
                DbValue::TinyintUnsigned(v) => {
                    add_node(target, SerializableDbValueNode::TinyintUnsigned(v))
                }
                DbValue::SmallintUnsigned(v) => {
                    add_node(target, SerializableDbValueNode::SmallintUnsigned(v))
                }
                DbValue::MediumintUnsigned(v) => {
                    add_node(target, SerializableDbValueNode::MediumintUnsigned(v))
                }
                DbValue::IntUnsigned(v) => {
                    add_node(target, SerializableDbValueNode::IntUnsigned(v))
                }
                DbValue::BigintUnsigned(v) => {
                    add_node(target, SerializableDbValueNode::BigintUnsigned(v))
                }
                DbValue::Float(v) => add_node(target, SerializableDbValueNode::Float(v)),
                DbValue::Double(v) => add_node(target, SerializableDbValueNode::Double(v)),
                DbValue::Decimal(v) => add_node(target, SerializableDbValueNode::Decimal(v)),
                DbValue::Date(v) => add_node(target, SerializableDbValueNode::Date(v)),
                DbValue::Datetime(v) => add_node(target, SerializableDbValueNode::Datetimetz(v)),
                DbValue::Timestamp(v) => add_node(target, SerializableDbValueNode::Timestamptz(v)),
                DbValue::Time(v) => add_node(target, SerializableDbValueNode::Time(v)),
                DbValue::Year(v) => add_node(target, SerializableDbValueNode::Year(v)),
                DbValue::Fixchar(v) => add_node(target, SerializableDbValueNode::Bpchar(v)),
                DbValue::Varchar(v) => add_node(target, SerializableDbValueNode::Varchar(v)),
                DbValue::Tinytext(v) => add_node(target, SerializableDbValueNode::Tinytext(v)),
                DbValue::Text(v) => add_node(target, SerializableDbValueNode::Text(v)),
                DbValue::Mediumtext(v) => add_node(target, SerializableDbValueNode::Mediumtext(v)),
                DbValue::Longtext(v) => add_node(target, SerializableDbValueNode::Longtext(v)),
                DbValue::Binary(v) => add_node(target, SerializableDbValueNode::Binary(v)),
                DbValue::Varbinary(v) => add_node(target, SerializableDbValueNode::Varbinary(v)),
                DbValue::Tinyblob(v) => add_node(target, SerializableDbValueNode::Tinyblob(v)),
                DbValue::Blob(v) => add_node(target, SerializableDbValueNode::Blob(v)),
                DbValue::Mediumblob(v) => add_node(target, SerializableDbValueNode::Mediumblob(v)),
                DbValue::Longblob(v) => add_node(target, SerializableDbValueNode::Longblob(v)),
                DbValue::Enumeration(v) => add_node(
                    target,
                    SerializableDbValueNode::Enumeration(
                        golem_common::model::oplog::payload::types::Enumeration {
                            name: "".to_string(),
                            value: v,
                        },
                    ),
                ),
                DbValue::Set(v) => add_node(target, SerializableDbValueNode::Set(v)),
                DbValue::Bit(v) => add_node(target, SerializableDbValueNode::Bit(v)),
                DbValue::Json(v) => add_node(target, SerializableDbValueNode::Json(v)),
                DbValue::Null => add_node(target, SerializableDbValueNode::Null),
            }
        }

        let mut result = SerializableDbValue { nodes: vec![] };
        add_db_value(&mut result, value);
        result
    }
}

impl TryFrom<SerializableDbValue> for DbValue {
    type Error = String;

    fn try_from(mut value: SerializableDbValue) -> Result<Self, Self::Error> {
        if value.nodes.len() != 1 {
            return Err("SerializableDbValue must have exactly one node".to_string());
        }
        let node = value.nodes.remove(0);
        match node {
            SerializableDbValueNode::Boolean(v) => Ok(DbValue::Boolean(v)),
            SerializableDbValueNode::Tinyint(v) => Ok(DbValue::Tinyint(v)),
            SerializableDbValueNode::Smallint(v) => Ok(DbValue::Smallint(v)),
            SerializableDbValueNode::Mediumint(v) => Ok(DbValue::Mediumint(v)),
            SerializableDbValueNode::Int(v) => Ok(DbValue::Int(v)),
            SerializableDbValueNode::Bigint(v) => Ok(DbValue::Bigint(v)),
            SerializableDbValueNode::TinyintUnsigned(v) => Ok(DbValue::TinyintUnsigned(v)),
            SerializableDbValueNode::SmallintUnsigned(v) => Ok(DbValue::SmallintUnsigned(v)),
            SerializableDbValueNode::MediumintUnsigned(v) => Ok(DbValue::MediumintUnsigned(v)),
            SerializableDbValueNode::IntUnsigned(v) => Ok(DbValue::IntUnsigned(v)),
            SerializableDbValueNode::BigintUnsigned(v) => Ok(DbValue::BigintUnsigned(v)),
            SerializableDbValueNode::Float(v) => Ok(DbValue::Float(v)),
            SerializableDbValueNode::Double(v) => Ok(DbValue::Double(v)),
            SerializableDbValueNode::Decimal(v) => Ok(DbValue::Decimal(v)),
            SerializableDbValueNode::Date(v) => Ok(DbValue::Date(v)),
            SerializableDbValueNode::Datetimetz(v) => Ok(DbValue::Datetime(v)),
            SerializableDbValueNode::Timestamptz(v) => Ok(DbValue::Timestamp(v)),
            SerializableDbValueNode::Time(v) => Ok(DbValue::Time(v)),
            SerializableDbValueNode::Year(v) => Ok(DbValue::Year(v)),
            SerializableDbValueNode::Bpchar(v) => Ok(DbValue::Fixchar(v)),
            SerializableDbValueNode::Varchar(v) => Ok(DbValue::Varchar(v)),
            SerializableDbValueNode::Tinytext(v) => Ok(DbValue::Tinytext(v)),
            SerializableDbValueNode::Text(v) => Ok(DbValue::Text(v)),
            SerializableDbValueNode::Mediumtext(v) => Ok(DbValue::Mediumtext(v)),
            SerializableDbValueNode::Longtext(v) => Ok(DbValue::Longtext(v)),
            SerializableDbValueNode::Binary(v) => Ok(DbValue::Binary(v)),
            SerializableDbValueNode::Varbinary(v) => Ok(DbValue::Varbinary(v)),
            SerializableDbValueNode::Tinyblob(v) => Ok(DbValue::Tinyblob(v)),
            SerializableDbValueNode::Blob(v) => Ok(DbValue::Blob(v)),
            SerializableDbValueNode::Mediumblob(v) => Ok(DbValue::Mediumblob(v)),
            SerializableDbValueNode::Longblob(v) => Ok(DbValue::Longblob(v)),
            SerializableDbValueNode::Enumeration(e) => Ok(DbValue::Enumeration(e.value)),
            SerializableDbValueNode::Set(v) => Ok(DbValue::Set(v)),
            SerializableDbValueNode::Bit(v) => Ok(DbValue::Bit(v)),
            SerializableDbValueNode::Json(v) => Ok(DbValue::Json(v)),
            SerializableDbValueNode::Null => Ok(DbValue::Null),
            _ => Err(format!(
                "Unsupported SerializableDbValueNode variant for MySQL: {:?}",
                value
            )),
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

impl From<DbColumn> for SerializableDbColumn {
    fn from(value: DbColumn) -> Self {
        Self {
            ordinal: value.ordinal,
            name: value.name,
            db_type: value.db_type.into(),
            db_type_name: value.db_type_name,
        }
    }
}

impl TryFrom<SerializableDbColumn> for DbColumn {
    type Error = String;

    fn try_from(value: SerializableDbColumn) -> Result<Self, Self::Error> {
        Ok(DbColumn {
            ordinal: value.ordinal,
            name: value.name,
            db_type: value.db_type.try_into()?,
            db_type_name: value.db_type_name,
        })
    }
}

#[cfg(test)]
pub mod tests {
    use crate::services::rdbms::mysql::types as mysql_types;
    use bigdecimal::BigDecimal;
    use bit_vec::BitVec;
    use serde_json::json;
    use std::str::FromStr;
    use uuid::Uuid;

    pub(crate) fn get_test_db_column_types() -> Vec<mysql_types::DbColumnType> {
        vec![
            mysql_types::DbColumnType::Boolean,
            mysql_types::DbColumnType::Tinyint,
            mysql_types::DbColumnType::Smallint,
            mysql_types::DbColumnType::Mediumint,
            mysql_types::DbColumnType::Int,
            mysql_types::DbColumnType::Bigint,
            mysql_types::DbColumnType::TinyintUnsigned,
            mysql_types::DbColumnType::SmallintUnsigned,
            mysql_types::DbColumnType::MediumintUnsigned,
            mysql_types::DbColumnType::IntUnsigned,
            mysql_types::DbColumnType::BigintUnsigned,
            mysql_types::DbColumnType::Float,
            mysql_types::DbColumnType::Double,
            mysql_types::DbColumnType::Decimal,
            mysql_types::DbColumnType::Date,
            mysql_types::DbColumnType::Datetime,
            mysql_types::DbColumnType::Timestamp,
            mysql_types::DbColumnType::Time,
            mysql_types::DbColumnType::Year,
            mysql_types::DbColumnType::Fixchar,
            mysql_types::DbColumnType::Varchar,
            mysql_types::DbColumnType::Tinytext,
            mysql_types::DbColumnType::Text,
            mysql_types::DbColumnType::Mediumtext,
            mysql_types::DbColumnType::Longtext,
            mysql_types::DbColumnType::Binary,
            mysql_types::DbColumnType::Varbinary,
            mysql_types::DbColumnType::Tinyblob,
            mysql_types::DbColumnType::Blob,
            mysql_types::DbColumnType::Mediumblob,
            mysql_types::DbColumnType::Longblob,
            mysql_types::DbColumnType::Enumeration,
            mysql_types::DbColumnType::Set,
            mysql_types::DbColumnType::Bit,
            mysql_types::DbColumnType::Json,
        ]
    }

    pub(crate) fn get_test_db_values() -> Vec<mysql_types::DbValue> {
        vec![
            mysql_types::DbValue::Tinyint(1),
            mysql_types::DbValue::Smallint(2),
            mysql_types::DbValue::Mediumint(3),
            mysql_types::DbValue::Int(4),
            mysql_types::DbValue::Bigint(5),
            mysql_types::DbValue::Float(6.0),
            mysql_types::DbValue::Double(7.0),
            mysql_types::DbValue::Decimal(BigDecimal::from_str("80.00").unwrap()),
            mysql_types::DbValue::Date(chrono::NaiveDate::from_ymd_opt(2030, 10, 12).unwrap()),
            mysql_types::DbValue::Datetime(chrono::DateTime::from_naive_utc_and_offset(
                chrono::NaiveDateTime::new(
                    chrono::NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
                    chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                ),
                chrono::Utc,
            )),
            mysql_types::DbValue::Timestamp(chrono::DateTime::from_naive_utc_and_offset(
                chrono::NaiveDateTime::new(
                    chrono::NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
                    chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                ),
                chrono::Utc,
            )),
            mysql_types::DbValue::Fixchar("0123456789".to_string()),
            mysql_types::DbValue::Varchar(format!("name-{}", Uuid::new_v4())),
            mysql_types::DbValue::Tinytext("Tinytext".to_string()),
            mysql_types::DbValue::Text("text".to_string()),
            mysql_types::DbValue::Mediumtext("Mediumtext".to_string()),
            mysql_types::DbValue::Longtext("Longtext".to_string()),
            mysql_types::DbValue::Binary(vec![66, 105, 110, 97, 114, 121]),
            mysql_types::DbValue::Varbinary("Varbinary".as_bytes().to_vec()),
            mysql_types::DbValue::Tinyblob("Tinyblob".as_bytes().to_vec()),
            mysql_types::DbValue::Blob("Blob".as_bytes().to_vec()),
            mysql_types::DbValue::Mediumblob("Mediumblob".as_bytes().to_vec()),
            mysql_types::DbValue::Longblob("Longblob".as_bytes().to_vec()),
            mysql_types::DbValue::Enumeration("value2".to_string()),
            mysql_types::DbValue::Set("value1,value2".to_string()),
            mysql_types::DbValue::Json(
                json!(
                       {
                          "id": 100
                       }
                )
                .to_string(),
            ),
            mysql_types::DbValue::Bit(BitVec::from_iter([true, false, false])),
            mysql_types::DbValue::TinyintUnsigned(10),
            mysql_types::DbValue::SmallintUnsigned(20),
            mysql_types::DbValue::MediumintUnsigned(30),
            mysql_types::DbValue::IntUnsigned(40),
            mysql_types::DbValue::BigintUnsigned(50),
            mysql_types::DbValue::Year(2020),
            mysql_types::DbValue::Time(chrono::NaiveTime::from_hms_opt(1, 20, 30).unwrap()),
        ]
    }

    mod roundtrip_tests {
        use super::super::*;
        use golem_common::model::oplog::payload::types::SerializableDbValue;
        use serde_json::json;
        use std::str::FromStr;
        use test_r::test;

        #[test]
        fn test_dbvalue_roundtrip_simple_types() {
            let test_values = vec![
                DbValue::Int(42),
                DbValue::Varchar("hello".to_string()),
                DbValue::Boolean(true),
                DbValue::Null,
            ];

            for original in test_values {
                let serialized: SerializableDbValue = original.clone().into();
                let deserialized: DbValue = serialized.try_into().expect("deserialization failed");
                assert_eq!(
                    original, deserialized,
                    "roundtrip failed for {:?}",
                    original
                );
            }
        }

        #[test]
        fn test_dbvalue_roundtrip_numeric_types() {
            let test_values = vec![
                DbValue::Tinyint(1),
                DbValue::Smallint(2),
                DbValue::Mediumint(3),
                DbValue::Bigint(5),
                DbValue::Float(6.0),
                DbValue::Double(7.0),
                DbValue::Decimal(BigDecimal::from_str("80.00").unwrap()),
                DbValue::TinyintUnsigned(10),
                DbValue::SmallintUnsigned(20),
                DbValue::MediumintUnsigned(30),
                DbValue::IntUnsigned(40),
                DbValue::BigintUnsigned(50),
            ];

            for value in test_values {
                let serialized: SerializableDbValue = value.clone().into();
                let deserialized: DbValue = serialized.try_into().expect("deserialization failed");
                assert_eq!(value, deserialized, "roundtrip failed for numeric type");
            }
        }

        #[test]
        fn test_dbvalue_roundtrip_temporal_types() {
            let test_values = vec![
                DbValue::Date(chrono::NaiveDate::from_ymd_opt(2030, 10, 12).unwrap()),
                DbValue::Datetime(chrono::DateTime::from_naive_utc_and_offset(
                    chrono::NaiveDateTime::new(
                        chrono::NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
                        chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                    ),
                    chrono::Utc,
                )),
                DbValue::Timestamp(chrono::DateTime::from_naive_utc_and_offset(
                    chrono::NaiveDateTime::new(
                        chrono::NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
                        chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                    ),
                    chrono::Utc,
                )),
                DbValue::Time(chrono::NaiveTime::from_hms_opt(1, 20, 30).unwrap()),
                DbValue::Year(2020),
            ];

            for original in test_values {
                let serialized: SerializableDbValue = original.clone().into();
                let deserialized: DbValue = serialized.try_into().expect("deserialization failed");
                assert_eq!(original, deserialized, "roundtrip failed for temporal type");
            }
        }

        #[test]
        fn test_dbvalue_roundtrip_string_types() {
            let test_values = vec![
                DbValue::Fixchar("fixed".to_string()),
                DbValue::Varchar("varchar".to_string()),
                DbValue::Tinytext("tiny".to_string()),
                DbValue::Text("text".to_string()),
                DbValue::Mediumtext("medium".to_string()),
                DbValue::Longtext("long".to_string()),
                DbValue::Enumeration("enum_value".to_string()),
                DbValue::Set("set_value".to_string()),
                DbValue::Json(json!({"key": "value"}).to_string()),
            ];

            for original in test_values {
                let serialized: SerializableDbValue = original.clone().into();
                let deserialized: DbValue = serialized.try_into().expect("deserialization failed");
                assert_eq!(original, deserialized, "roundtrip failed for string type");
            }
        }

        #[test]
        fn test_dbvalue_roundtrip_binary_types() {
            let test_values = vec![
                DbValue::Binary(vec![1, 2, 3]),
                DbValue::Varbinary(vec![4, 5, 6]),
                DbValue::Tinyblob(vec![7, 8, 9]),
                DbValue::Blob(vec![10, 11, 12]),
                DbValue::Mediumblob(vec![13, 14, 15]),
                DbValue::Longblob(vec![16, 17, 18]),
                DbValue::Bit(BitVec::from_iter([true, false, true])),
            ];

            for original in test_values {
                let serialized: SerializableDbValue = original.clone().into();
                let deserialized: DbValue = serialized.try_into().expect("deserialization failed");
                assert_eq!(original, deserialized, "roundtrip failed for binary type");
            }
        }

        #[test]
        fn test_dbvalue_roundtrip_all_test_values() {
            let test_values = super::get_test_db_values();

            for (idx, original) in test_values.into_iter().enumerate() {
                let serialized: SerializableDbValue = original.clone().into();
                let deserialized: DbValue = serialized
                    .try_into()
                    .expect(&format!("deserialization failed for test value {}", idx));
                assert_eq!(
                    original, deserialized,
                    "roundtrip failed for test value at index {}",
                    idx
                );
            }
        }
    }
}
