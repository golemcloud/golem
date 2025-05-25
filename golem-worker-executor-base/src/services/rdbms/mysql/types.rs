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

use crate::services::rdbms::{AnalysedTypeMerger, RdbmsIntoValueAndType};
use bigdecimal::BigDecimal;
use bincode::{Decode, Encode};
use bit_vec::BitVec;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::{IntoValue, IntoValueAndType, ValueAndType};
use golem_wasm_rpc_derive::IntoValue;
use std::fmt::Display;

#[derive(Clone, Debug, Eq, PartialEq, Encode, Decode, IntoValue)]
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

impl RdbmsIntoValueAndType for DbColumnType {
    fn into_value_and_type(self) -> ValueAndType {
        IntoValueAndType::into_value_and_type(self)
    }

    fn get_base_type() -> AnalysedType {
        DbColumnType::get_type()
    }
}

#[derive(Clone, Debug, PartialEq, Encode, Decode, IntoValue)]
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
    Decimal(#[bincode(with_serde)] BigDecimal),
    Date(#[bincode(with_serde)] chrono::NaiveDate),
    Datetime(#[bincode(with_serde)] chrono::DateTime<chrono::Utc>),
    Timestamp(#[bincode(with_serde)] chrono::DateTime<chrono::Utc>),
    Time(#[bincode(with_serde)] chrono::NaiveTime),
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
    Bit(#[bincode(with_serde)] BitVec),
    Json(String),
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

impl RdbmsIntoValueAndType for DbValue {
    fn into_value_and_type(self) -> ValueAndType {
        IntoValueAndType::into_value_and_type(self)
    }

    fn get_base_type() -> AnalysedType {
        DbValue::get_type()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Encode, Decode, IntoValue)]
pub struct DbColumn {
    pub ordinal: u64,
    pub name: String,
    pub db_type: DbColumnType,
    pub db_type_name: String,
}

impl RdbmsIntoValueAndType for DbColumn {
    fn into_value_and_type(self) -> ValueAndType {
        IntoValueAndType::into_value_and_type(self)
    }

    fn get_base_type() -> AnalysedType {
        DbColumn::get_type()
    }
}

impl AnalysedTypeMerger for DbColumnType {
    fn merge_types(first: AnalysedType, _second: AnalysedType) -> AnalysedType {
        // same types are expected
        first
    }
}

impl AnalysedTypeMerger for DbValue {
    fn merge_types(first: AnalysedType, _second: AnalysedType) -> AnalysedType {
        // same types are expected
        first
    }
}

impl AnalysedTypeMerger for DbColumn {
    fn merge_types(first: AnalysedType, _second: AnalysedType) -> AnalysedType {
        // same types are expected
        first
    }
}

#[cfg(test)]
pub mod tests {
    use crate::services::rdbms::mysql::{types as mysql_types, MysqlType};
    use crate::services::rdbms::{DbResult, DbRow, RdbmsIntoValueAndType};
    use assert2::check;
    use bigdecimal::BigDecimal;
    use bincode::{Decode, Encode};
    use bit_vec::BitVec;
    use golem_common::serialization::{serialize, try_deserialize};
    use serde_json::json;
    use std::str::FromStr;
    use test_r::test;
    use uuid::Uuid;

    fn check_bincode<T: Encode + Decode + PartialEq>(value: T) {
        let bin_value = serialize(&value).unwrap().to_vec();
        let value2: Option<T> = try_deserialize(bin_value.as_slice()).ok().flatten();
        check!(value2.unwrap() == value);
    }

    fn check_type_and_value<T: RdbmsIntoValueAndType>(value: T) {
        let value_and_type = value.into_value_and_type();
        let value_and_type_json = serde_json::to_string(&value_and_type);
        check!(value_and_type_json.is_ok());
    }

    #[test]
    fn test_db_values_conversions() {
        for value in get_test_db_values() {
            check_bincode(value.clone());
            check_type_and_value(value);
        }
    }

    #[test]
    fn test_db_column_types_conversions() {
        for value in get_test_db_column_types() {
            check_bincode(value.clone());
            check_type_and_value(value);
        }
    }

    #[test]
    fn test_db_result_conversions() {
        let value = DbResult::<MysqlType>::new(
            get_test_db_columns(),
            vec![DbRow {
                values: get_test_db_values(),
            }],
        );

        check_bincode(value.clone());
        check_type_and_value(value);
    }

    pub(crate) fn get_test_db_columns() -> Vec<mysql_types::DbColumn> {
        let types = get_test_db_column_types();
        let mut columns: Vec<mysql_types::DbColumn> = Vec::with_capacity(types.len());

        for (i, ct) in types.iter().enumerate() {
            let c = mysql_types::DbColumn {
                ordinal: i as u64,
                name: format!("column-{}", i),
                db_type: ct.clone(),
                db_type_name: ct.to_string(),
            };
            columns.push(c);
        }
        columns
    }

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
}
