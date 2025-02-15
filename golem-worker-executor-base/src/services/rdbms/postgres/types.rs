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

use bigdecimal::BigDecimal;
use bincode::{Decode, Encode};
use bit_vec::BitVec;
use golem_wasm_ast::analysis::{analysed_type, AnalysedType};
use golem_wasm_rpc::{IntoValue, Value};
use itertools::Itertools;
use mac_address::MacAddress;
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display};
use std::net::IpAddr;
use std::ops::Bound;
use uuid::Uuid;

pub trait NamedType {
    fn name(&self) -> String;
}

#[derive(Clone, Debug, Eq, PartialEq, Encode, Decode)]
pub struct EnumerationType {
    pub name: String,
}

impl EnumerationType {
    pub fn new(name: String) -> Self {
        EnumerationType { name }
    }
}

impl NamedType for EnumerationType {
    fn name(&self) -> String {
        self.name.clone()
    }
}

impl Display for EnumerationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl IntoValue for EnumerationType {
    fn into_value(self) -> Value {
        Value::Record(vec![self.name.into_value()])
    }

    fn get_type() -> AnalysedType {
        analysed_type::record(vec![analysed_type::field("name", analysed_type::str())])
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Encode, Decode)]
pub struct CompositeType {
    pub name: String,
    pub attributes: Vec<(String, DbColumnType)>,
}

impl CompositeType {
    pub fn new(name: String, attributes: Vec<(String, DbColumnType)>) -> Self {
        CompositeType { name, attributes }
    }
}

impl Display for CompositeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}({})",
            self.name,
            self.attributes
                .iter()
                .map(|v| format!("{} {}", v.0, v.1))
                .format(", ")
        )
    }
}

impl NamedType for CompositeType {
    fn name(&self) -> String {
        self.name.clone()
    }
}

impl IntoValue for CompositeType {
    fn into_value(self) -> Value {
        Value::Record(vec![self.name.into_value(), self.attributes.into_value()])
    }

    fn get_type() -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("name", analysed_type::str()),
            analysed_type::field(
                "attributes",
                analysed_type::list(analysed_type::tuple(vec![
                    analysed_type::str(),
                    DbColumnType::get_type(),
                ])),
            ),
        ])
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Encode, Decode)]
pub struct DomainType {
    pub name: String,
    pub base_type: Box<DbColumnType>,
}

impl DomainType {
    pub fn new(name: String, base_type: DbColumnType) -> Self {
        DomainType {
            name,
            base_type: Box::new(base_type),
        }
    }
}

impl Display for DomainType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", self.name, self.base_type)
    }
}

impl NamedType for DomainType {
    fn name(&self) -> String {
        self.name.clone()
    }
}

impl IntoValue for DomainType {
    fn into_value(self) -> Value {
        Value::Record(vec![self.name.into_value(), (*self.base_type).into_value()])
    }

    fn get_type() -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("name", analysed_type::str()),
            analysed_type::field("base-type", DbColumnType::get_type()),
        ])
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Encode, Decode)]
pub struct RangeType {
    pub name: String,
    pub base_type: Box<DbColumnType>,
}

impl RangeType {
    pub fn new(name: String, base_type: DbColumnType) -> Self {
        RangeType {
            name,
            base_type: Box::new(base_type),
        }
    }
}

impl Display for RangeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", self.name, self.base_type)
    }
}

impl NamedType for RangeType {
    fn name(&self) -> String {
        self.name.clone()
    }
}

impl IntoValue for RangeType {
    fn into_value(self) -> Value {
        Value::Record(vec![self.name.into_value(), (*self.base_type).into_value()])
    }

    fn get_type() -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("name", analysed_type::str()),
            analysed_type::field("base-type", DbColumnType::get_type()),
        ])
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct ValuesRange<T> {
    pub start: Bound<T>,
    pub end: Bound<T>,
}

impl<T> ValuesRange<T> {
    pub fn new(start: Bound<T>, end: Bound<T>) -> Self {
        ValuesRange { start, end }
    }

    pub fn start_value(&self) -> Option<&T> {
        match &self.start {
            Bound::Included(v) => Some(v),
            Bound::Excluded(v) => Some(v),
            Bound::Unbounded => None,
        }
    }

    pub fn end_value(&self) -> Option<&T> {
        match &self.end {
            Bound::Included(v) => Some(v),
            Bound::Excluded(v) => Some(v),
            Bound::Unbounded => None,
        }
    }

    pub fn map<U>(self, f: impl Fn(T) -> U + Clone) -> ValuesRange<U> {
        let start: Bound<U> = self.start.map(f.clone());
        let end: Bound<U> = self.end.map(f.clone());
        ValuesRange::new(start, end)
    }

    pub fn try_map<U>(
        self,
        f: impl Fn(T) -> Result<U, String> + Clone,
    ) -> Result<ValuesRange<U>, String> {
        fn to_bound<T, U>(
            v: Bound<T>,
            f: impl Fn(T) -> Result<U, String>,
        ) -> Result<Bound<U>, String> {
            match v {
                Bound::Included(v) => Ok(Bound::Included(f(v)?)),
                Bound::Excluded(v) => Ok(Bound::Excluded(f(v)?)),
                Bound::Unbounded => Ok(Bound::Unbounded),
            }
        }
        let start: Bound<U> = to_bound(self.start, f.clone())?;
        let end: Bound<U> = to_bound(self.end, f.clone())?;

        Ok(ValuesRange::new(start, end))
    }
}

impl<T: Debug> Display for ValuesRange<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} {:?}", self.start, self.end)
    }
}

impl<T: IntoValue> IntoValue for ValuesRange<T> {
    fn into_value(self) -> Value {
        Value::Record(vec![self.start.into_value(), self.end.into_value()])
    }

    fn get_type() -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("start", Bound::<T>::get_type()),
            analysed_type::field("end", Bound::<T>::get_type()),
        ])
    }
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct Interval {
    pub months: i32,
    pub days: i32,
    pub microseconds: i64,
}

impl Interval {
    pub fn new(months: i32, days: i32, microseconds: i64) -> Self {
        Interval {
            months,
            days,
            microseconds,
        }
    }
}

impl Display for Interval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}m {}d {}us", self.months, self.days, self.microseconds)
    }
}

impl IntoValue for Interval {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.months.into_value(),
            self.days.into_value(),
            self.microseconds.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("months", analysed_type::s32()),
            analysed_type::field("days", analysed_type::s32()),
            analysed_type::field("microseconds", analysed_type::s64()),
        ])
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct TimeTz {
    #[bincode(with_serde)]
    pub time: chrono::NaiveTime,
    pub offset: i32,
}

impl TimeTz {
    pub fn new(time: chrono::NaiveTime, offset: chrono::FixedOffset) -> Self {
        TimeTz {
            time,
            offset: offset.utc_minus_local(),
        }
    }
}

impl Display for TimeTz {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.time, self.offset)
    }
}

impl IntoValue for TimeTz {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.time.to_string().into_value(),
            self.offset.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("time", analysed_type::str()),
            analysed_type::field("offset", analysed_type::s32()),
        ])
    }
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct Enumeration {
    pub name: String,
    pub value: String,
}

impl Enumeration {
    pub fn new(name: String, value: String) -> Self {
        Enumeration { name, value }
    }
}

impl NamedType for Enumeration {
    fn name(&self) -> String {
        self.name.clone()
    }
}

impl Display for Enumeration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", self.name, self.value)
    }
}

impl IntoValue for Enumeration {
    fn into_value(self) -> Value {
        Value::Record(vec![self.name.into_value(), self.value.into_value()])
    }

    fn get_type() -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("name", analysed_type::str()),
            analysed_type::field("value", analysed_type::str()),
        ])
    }
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct Composite {
    pub name: String,
    pub values: Vec<DbValue>,
}

impl Composite {
    pub fn new(name: String, values: Vec<DbValue>) -> Self {
        Composite { name, values }
    }
}

impl Display for Composite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}({})",
            self.name,
            self.values.iter().map(|v| format!("{}", v)).format(", ")
        )
    }
}

impl NamedType for Composite {
    fn name(&self) -> String {
        self.name.clone()
    }
}

impl IntoValue for Composite {
    fn into_value(self) -> Value {
        Value::Record(vec![self.name.into_value(), self.values.into_value()])
    }

    fn get_type() -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("name", analysed_type::str()),
            analysed_type::field("values", analysed_type::str()),
        ])
    }
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct Domain {
    pub name: String,
    pub value: Box<DbValue>,
}

impl Domain {
    pub fn new(name: String, value: DbValue) -> Self {
        Domain {
            name,
            value: Box::new(value),
        }
    }
}

impl NamedType for Domain {
    fn name(&self) -> String {
        self.name.clone()
    }
}

impl Display for Domain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", self.name, self.value)
    }
}

impl IntoValue for Domain {
    fn into_value(self) -> Value {
        Value::Record(vec![self.name.into_value(), self.value.into_value()])
    }

    fn get_type() -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("name", analysed_type::str()),
            analysed_type::field("value", DbValue::get_type()),
        ])
    }
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct Range {
    pub name: String,
    pub value: Box<ValuesRange<DbValue>>,
}

impl Range {
    pub fn new(name: String, value: ValuesRange<DbValue>) -> Self {
        Range {
            name,
            value: Box::new(value),
        }
    }
}

impl NamedType for Range {
    fn name(&self) -> String {
        self.name.clone()
    }
}

impl Display for Range {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", self.name, self.value)
    }
}

impl IntoValue for Range {
    fn into_value(self) -> Value {
        Value::Record(vec![self.name.into_value(), (*self.value).into_value()])
    }

    fn get_type() -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("name", analysed_type::str()),
            analysed_type::field("value", ValuesRange::<DbValue>::get_type()),
        ])
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Encode, Decode)]
pub enum DbColumnType {
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
    Jsonpath,
    Inet,
    Cidr,
    Macaddr,
    Bit,
    Varbit,
    Int4range,
    Int8range,
    Numrange,
    Tsrange,
    Tstzrange,
    Daterange,
    Money,
    Oid,
    Enumeration(EnumerationType),
    Composite(CompositeType),
    Domain(DomainType),
    Range(RangeType),
    Array(Box<DbColumnType>),
    Null,
}

impl DbColumnType {
    pub(crate) fn into_array(self) -> DbColumnType {
        if let DbColumnType::Array(_) = self {
            self
        } else {
            DbColumnType::Array(Box::new(self))
        }
    }

    pub fn is_complex_type(&self) -> bool {
        matches!(
            self,
            DbColumnType::Composite(_)
                | DbColumnType::Domain(_)
                | DbColumnType::Array(_)
                | DbColumnType::Range(_)
        )
    }
}

impl Display for DbColumnType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbColumnType::Character => write!(f, "character"),
            DbColumnType::Int2 => write!(f, "int2"),
            DbColumnType::Int4 => write!(f, "int4"),
            DbColumnType::Int8 => write!(f, "int8"),
            DbColumnType::Float4 => write!(f, "float4"),
            DbColumnType::Float8 => write!(f, "float8"),
            DbColumnType::Numeric => write!(f, "numeric"),
            DbColumnType::Boolean => write!(f, "boolean"),
            DbColumnType::Timestamp => write!(f, "timestamp"),
            DbColumnType::Date => write!(f, "date"),
            DbColumnType::Time => write!(f, "time"),
            DbColumnType::Timestamptz => write!(f, "timestamptz"),
            DbColumnType::Timetz => write!(f, "timetz"),
            DbColumnType::Interval => write!(f, "interval"),
            DbColumnType::Text => write!(f, "text"),
            DbColumnType::Varchar => write!(f, "varchar"),
            DbColumnType::Bpchar => write!(f, "bpchar"),
            DbColumnType::Bytea => write!(f, "bytea"),
            DbColumnType::Json => write!(f, "json"),
            DbColumnType::Jsonb => write!(f, "jsonb"),
            DbColumnType::Jsonpath => write!(f, "jsonpath"),
            DbColumnType::Xml => write!(f, "xml"),
            DbColumnType::Uuid => write!(f, "uuid"),
            DbColumnType::Inet => write!(f, "inet"),
            DbColumnType::Cidr => write!(f, "cidr"),
            DbColumnType::Macaddr => write!(f, "macaddr"),
            DbColumnType::Bit => write!(f, "bit"),
            DbColumnType::Varbit => write!(f, "varbit"),
            DbColumnType::Int4range => write!(f, "int4range"),
            DbColumnType::Int8range => write!(f, "int8range"),
            DbColumnType::Numrange => write!(f, "numrange"),
            DbColumnType::Tsrange => write!(f, "tsrange"),
            DbColumnType::Tstzrange => write!(f, "tstzrange"),
            DbColumnType::Daterange => write!(f, "daterange"),
            DbColumnType::Money => write!(f, "money"),
            DbColumnType::Oid => write!(f, "oid"),
            DbColumnType::Enumeration(v) => write!(f, "enumeration: {}", v),
            DbColumnType::Composite(v) => {
                write!(f, "composite: {}", v)
            }
            DbColumnType::Domain(v) => {
                write!(f, "domain: {}", v)
            }
            DbColumnType::Array(v) => {
                write!(f, "{}[]", v)
            }
            DbColumnType::Range(v) => {
                write!(f, "range: {}", v)
            }
            DbColumnType::Null => write!(f, "null"),
        }
    }
}

impl IntoValue for DbColumnType {
    fn into_value(self) -> Value {
        match self {
            DbColumnType::Character => Value::Variant {
                case_idx: 0,
                case_value: None,
            },
            DbColumnType::Int2 => Value::Variant {
                case_idx: 1,
                case_value: None,
            },
            DbColumnType::Int4 => Value::Variant {
                case_idx: 2,
                case_value: None,
            },
            DbColumnType::Int8 => Value::Variant {
                case_idx: 3,
                case_value: None,
            },
            DbColumnType::Float4 => Value::Variant {
                case_idx: 4,
                case_value: None,
            },
            DbColumnType::Float8 => Value::Variant {
                case_idx: 5,
                case_value: None,
            },
            DbColumnType::Numeric => Value::Variant {
                case_idx: 6,
                case_value: None,
            },
            DbColumnType::Boolean => Value::Variant {
                case_idx: 7,
                case_value: None,
            },
            DbColumnType::Text => Value::Variant {
                case_idx: 8,
                case_value: None,
            },
            DbColumnType::Varchar => Value::Variant {
                case_idx: 9,
                case_value: None,
            },
            DbColumnType::Bpchar => Value::Variant {
                case_idx: 10,
                case_value: None,
            },
            DbColumnType::Timestamp => Value::Variant {
                case_idx: 11,
                case_value: None,
            },
            DbColumnType::Timestamptz => Value::Variant {
                case_idx: 12,
                case_value: None,
            },
            DbColumnType::Date => Value::Variant {
                case_idx: 13,
                case_value: None,
            },
            DbColumnType::Time => Value::Variant {
                case_idx: 14,
                case_value: None,
            },
            DbColumnType::Timetz => Value::Variant {
                case_idx: 15,
                case_value: None,
            },
            DbColumnType::Interval => Value::Variant {
                case_idx: 16,
                case_value: None,
            },
            DbColumnType::Bytea => Value::Variant {
                case_idx: 17,
                case_value: None,
            },
            DbColumnType::Uuid => Value::Variant {
                case_idx: 18,
                case_value: None,
            },
            DbColumnType::Xml => Value::Variant {
                case_idx: 19,
                case_value: None,
            },
            DbColumnType::Json => Value::Variant {
                case_idx: 20,
                case_value: None,
            },
            DbColumnType::Jsonb => Value::Variant {
                case_idx: 21,
                case_value: None,
            },
            DbColumnType::Jsonpath => Value::Variant {
                case_idx: 22,
                case_value: None,
            },
            DbColumnType::Inet => Value::Variant {
                case_idx: 23,
                case_value: None,
            },
            DbColumnType::Cidr => Value::Variant {
                case_idx: 24,
                case_value: None,
            },
            DbColumnType::Macaddr => Value::Variant {
                case_idx: 25,
                case_value: None,
            },
            DbColumnType::Bit => Value::Variant {
                case_idx: 26,
                case_value: None,
            },
            DbColumnType::Varbit => Value::Variant {
                case_idx: 27,
                case_value: None,
            },
            DbColumnType::Int4range => Value::Variant {
                case_idx: 28,
                case_value: None,
            },
            DbColumnType::Int8range => Value::Variant {
                case_idx: 29,
                case_value: None,
            },
            DbColumnType::Numrange => Value::Variant {
                case_idx: 30,
                case_value: None,
            },
            DbColumnType::Tsrange => Value::Variant {
                case_idx: 31,
                case_value: None,
            },
            DbColumnType::Tstzrange => Value::Variant {
                case_idx: 32,
                case_value: None,
            },
            DbColumnType::Daterange => Value::Variant {
                case_idx: 33,
                case_value: None,
            },
            DbColumnType::Money => Value::Variant {
                case_idx: 34,
                case_value: None,
            },
            DbColumnType::Oid => Value::Variant {
                case_idx: 35,
                case_value: None,
            },
            DbColumnType::Enumeration(v) => Value::Variant {
                case_idx: 36,
                case_value: Some(Box::new(v.into_value())),
            },
            DbColumnType::Composite(v) => Value::Variant {
                case_idx: 37,
                case_value: Some(Box::new(v.into_value())),
            },
            DbColumnType::Domain(v) => Value::Variant {
                case_idx: 38,
                case_value: Some(Box::new(v.into_value())),
            },
            DbColumnType::Array(v) => Value::Variant {
                case_idx: 39,
                case_value: Some(Box::new(v.into_value())),
            },
            DbColumnType::Range(v) => Value::Variant {
                case_idx: 40,
                case_value: Some(Box::new(v.into_value())),
            },
            DbColumnType::Null => Value::Variant {
                case_idx: 41,
                case_value: None,
            },
        }
    }

    fn get_type() -> AnalysedType {
        fn get_tpe(root: bool) -> AnalysedType {
            let array_type = if root {
                analysed_type::case("array", get_tpe(false))
            } else {
                analysed_type::unit_case("array")
            };

            analysed_type::variant(vec![
                analysed_type::unit_case("character"),
                analysed_type::unit_case("int2"),
                analysed_type::unit_case("int4"),
                analysed_type::unit_case("int8"),
                analysed_type::unit_case("float4"),
                analysed_type::unit_case("float8"),
                analysed_type::unit_case("numeric"),
                analysed_type::unit_case("boolean"),
                analysed_type::unit_case("text"),
                analysed_type::unit_case("varchar"),
                analysed_type::unit_case("bpchar"),
                analysed_type::unit_case("timestamp"),
                analysed_type::unit_case("timestamptz"),
                analysed_type::unit_case("date"),
                analysed_type::unit_case("time"),
                analysed_type::unit_case("timetz"),
                analysed_type::unit_case("interval"),
                analysed_type::unit_case("bytea"),
                analysed_type::unit_case("uuid"),
                analysed_type::unit_case("xml"),
                analysed_type::unit_case("json"),
                analysed_type::unit_case("jsonb"),
                analysed_type::unit_case("jsonpath"),
                analysed_type::unit_case("inet"),
                analysed_type::unit_case("cidr"),
                analysed_type::unit_case("macaddr"),
                analysed_type::unit_case("bit"),
                analysed_type::unit_case("varbit"),
                analysed_type::unit_case("int4range"),
                analysed_type::unit_case("int8range"),
                analysed_type::unit_case("numrange"),
                analysed_type::unit_case("tsrange"),
                analysed_type::unit_case("tstzrange"),
                analysed_type::unit_case("daterange"),
                analysed_type::unit_case("money"),
                analysed_type::unit_case("oid"),
                analysed_type::case("enumeration", EnumerationType::get_type()),
                analysed_type::case("composite", CompositeType::get_type()),
                analysed_type::case("domain", DomainType::get_type()),
                array_type,
                analysed_type::case("range", RangeType::get_type()),
                analysed_type::unit_case("null"),
            ])
        }
        get_tpe(true)
    }
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum DbValue {
    Character(i8),
    Int2(i16),
    Int4(i32),
    Int8(i64),
    Float4(f32),
    Float8(f64),
    Numeric(#[bincode(with_serde)] BigDecimal),
    Boolean(bool),
    Timestamp(#[bincode(with_serde)] chrono::NaiveDateTime),
    Timestamptz(#[bincode(with_serde)] chrono::DateTime<chrono::Utc>),
    Date(#[bincode(with_serde)] chrono::NaiveDate),
    Time(#[bincode(with_serde)] chrono::NaiveTime),
    Timetz(TimeTz),
    Interval(Interval),
    Text(String),
    Varchar(String),
    Bpchar(String),
    Bytea(Vec<u8>),
    Json(String),
    Jsonb(String),
    Jsonpath(String),
    Xml(String),
    Uuid(#[bincode(with_serde)] Uuid),
    Inet(#[bincode(with_serde)] IpAddr),
    Cidr(#[bincode(with_serde)] IpAddr),
    Macaddr(#[bincode(with_serde)] MacAddress),
    Bit(#[bincode(with_serde)] BitVec),
    Varbit(#[bincode(with_serde)] BitVec),
    Int4range(ValuesRange<i32>),
    Int8range(ValuesRange<i64>),
    Numrange(#[bincode(with_serde)] ValuesRange<BigDecimal>),
    Tsrange(#[bincode(with_serde)] ValuesRange<chrono::NaiveDateTime>),
    Tstzrange(#[bincode(with_serde)] ValuesRange<chrono::DateTime<chrono::Utc>>),
    Daterange(#[bincode(with_serde)] ValuesRange<chrono::NaiveDate>),
    Money(i64),
    Oid(u32),
    Enumeration(Enumeration),
    Composite(Composite),
    Domain(Domain),
    Range(Range),
    Array(Vec<DbValue>),
    Null,
}

impl Display for DbValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbValue::Character(v) => write!(f, "{}", v),
            DbValue::Int2(v) => write!(f, "{}", v),
            DbValue::Int4(v) => write!(f, "{}", v),
            DbValue::Int8(v) => write!(f, "{}", v),
            DbValue::Float4(v) => write!(f, "{}", v),
            DbValue::Float8(v) => write!(f, "{}", v),
            DbValue::Numeric(v) => write!(f, "{}", v),
            DbValue::Boolean(v) => write!(f, "{}", v),
            DbValue::Timestamp(v) => write!(f, "{}", v),
            DbValue::Timestamptz(v) => write!(f, "{}", v),
            DbValue::Date(v) => write!(f, "{}", v),
            DbValue::Time(v) => write!(f, "{}", v),
            DbValue::Timetz(v) => write!(f, "{}", v),
            DbValue::Interval(v) => write!(f, "{}", v),
            DbValue::Text(v) => write!(f, "{}", v),
            DbValue::Varchar(v) => write!(f, "{}", v),
            DbValue::Bpchar(v) => write!(f, "{}", v),
            DbValue::Bytea(v) => write!(f, "{:?}", v),
            DbValue::Json(v) => write!(f, "{}", v),
            DbValue::Jsonb(v) => write!(f, "{}", v),
            DbValue::Jsonpath(v) => write!(f, "{}", v),
            DbValue::Xml(v) => write!(f, "{}", v),
            DbValue::Uuid(v) => write!(f, "{}", v),
            DbValue::Inet(v) => write!(f, "{}", v),
            DbValue::Cidr(v) => write!(f, "{}", v),
            DbValue::Macaddr(v) => write!(f, "{}", v),
            DbValue::Bit(v) => write!(f, "{:?}", v),
            DbValue::Varbit(v) => write!(f, "{:?}", v),
            DbValue::Int4range(v) => write!(f, "{}", v),
            DbValue::Int8range(v) => write!(f, "{}", v),
            DbValue::Numrange(v) => write!(f, "{}", v),
            DbValue::Tsrange(v) => write!(f, "{}", v),
            DbValue::Tstzrange(v) => write!(f, "{}", v),
            DbValue::Daterange(v) => write!(f, "{}", v),
            DbValue::Oid(v) => write!(f, "{}", v),
            DbValue::Money(v) => write!(f, "{}", v),
            DbValue::Enumeration(v) => write!(f, "{}", v),
            DbValue::Composite(v) => write!(f, "{}", v),
            DbValue::Domain(v) => write!(f, "{}", v),
            DbValue::Array(v) => write!(f, "[{}]", v.iter().format(", ")),
            DbValue::Range(v) => write!(f, "{}", v),
            DbValue::Null => write!(f, "NULL"),
        }
    }
}

impl DbValue {
    pub(crate) fn array_from<T>(value: Option<Vec<T>>, f: impl Fn(T) -> DbValue) -> Self {
        match value {
            Some(v) => DbValue::Array(v.into_iter().map(f).collect()),
            None => DbValue::Array(vec![]),
        }
    }

    pub(crate) fn primitive_from<T>(value: Option<T>, f: impl Fn(T) -> DbValue) -> Self {
        match value {
            Some(v) => f(v),
            None => DbValue::Null,
        }
    }

    pub fn is_complex_type(&self) -> bool {
        matches!(
            self,
            DbValue::Composite(_) | DbValue::Domain(_) | DbValue::Array(_) | DbValue::Range(_)
        )
    }

    pub(crate) fn get_column_type(&self) -> DbColumnType {
        match self {
            DbValue::Character(_) => DbColumnType::Character,
            DbValue::Int2(_) => DbColumnType::Int2,
            DbValue::Int4(_) => DbColumnType::Int4,
            DbValue::Int8(_) => DbColumnType::Int8,
            DbValue::Float4(_) => DbColumnType::Float4,
            DbValue::Float8(_) => DbColumnType::Float8,
            DbValue::Numeric(_) => DbColumnType::Numeric,
            DbValue::Boolean(_) => DbColumnType::Boolean,
            DbValue::Text(_) => DbColumnType::Text,
            DbValue::Varchar(_) => DbColumnType::Varchar,
            DbValue::Bpchar(_) => DbColumnType::Bpchar,
            DbValue::Bytea(_) => DbColumnType::Bytea,
            DbValue::Uuid(_) => DbColumnType::Uuid,
            DbValue::Json(_) => DbColumnType::Json,
            DbValue::Jsonb(_) => DbColumnType::Jsonb,
            DbValue::Jsonpath(_) => DbColumnType::Jsonpath,
            DbValue::Xml(_) => DbColumnType::Xml,
            DbValue::Timestamp(_) => DbColumnType::Timestamp,
            DbValue::Timestamptz(_) => DbColumnType::Timestamptz,
            DbValue::Time(_) => DbColumnType::Time,
            DbValue::Timetz(_) => DbColumnType::Timetz,
            DbValue::Date(_) => DbColumnType::Date,
            DbValue::Interval(_) => DbColumnType::Interval,
            DbValue::Inet(_) => DbColumnType::Inet,
            DbValue::Cidr(_) => DbColumnType::Cidr,
            DbValue::Macaddr(_) => DbColumnType::Macaddr,
            DbValue::Bit(_) => DbColumnType::Bit,
            DbValue::Varbit(_) => DbColumnType::Varbit,
            DbValue::Int4range(_) => DbColumnType::Int4range,
            DbValue::Int8range(_) => DbColumnType::Int8range,
            DbValue::Numrange(_) => DbColumnType::Numrange,
            DbValue::Tsrange(_) => DbColumnType::Tsrange,
            DbValue::Tstzrange(_) => DbColumnType::Tstzrange,
            DbValue::Daterange(_) => DbColumnType::Daterange,
            DbValue::Money(_) => DbColumnType::Money,
            DbValue::Oid(_) => DbColumnType::Oid,
            DbValue::Enumeration(v) => {
                DbColumnType::Enumeration(EnumerationType::new(v.name.clone()))
            }
            DbValue::Composite(v) => {
                DbColumnType::Composite(CompositeType::new(v.name.clone(), vec![]))
            }
            DbValue::Domain(v) => {
                let t = v.value.get_column_type();
                DbColumnType::Domain(DomainType::new(v.name.clone(), t))
            }
            DbValue::Range(r) => {
                let v = (*r.value).start_value().or((*r.value).end_value());
                let t = v.map(|v| v.get_column_type()).unwrap_or(DbColumnType::Null);
                DbColumnType::Range(RangeType::new(r.name.clone(), t))
            }
            DbValue::Array(vs) => {
                let t = if !vs.is_empty() {
                    let t = vs[0].get_column_type();
                    match t {
                        DbColumnType::Range(r) if *r.base_type == DbColumnType::Null => {
                            let v = vs
                                .iter()
                                .map(|v| {
                                    if let DbValue::Range(v) = v {
                                        (*v.value).start_value().or((*v.value).end_value())
                                    } else {
                                        None
                                    }
                                })
                                .find(|v| v.is_some())
                                .flatten();
                            let t = v.map(|v| v.get_column_type()).unwrap_or(DbColumnType::Null);
                            DbColumnType::Range(RangeType::new(r.name, t))
                        }
                        _ => t,
                    }
                } else {
                    DbColumnType::Null
                };
                DbColumnType::Array(Box::new(t))
            }
            DbValue::Null => DbColumnType::Null,
        }
    }
}

impl IntoValue for DbValue {
    fn into_value(self) -> Value {
        match self {
            DbValue::Character(v) => Value::Variant {
                case_idx: 0,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Int2(v) => Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Int4(v) => Value::Variant {
                case_idx: 2,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Int8(v) => Value::Variant {
                case_idx: 3,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Float4(v) => Value::Variant {
                case_idx: 4,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Float8(v) => Value::Variant {
                case_idx: 5,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Numeric(v) => Value::Variant {
                case_idx: 6,
                case_value: Some(Box::new(v.to_string().into_value())),
            },
            DbValue::Boolean(v) => Value::Variant {
                case_idx: 7,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Text(v) => Value::Variant {
                case_idx: 8,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Varchar(v) => Value::Variant {
                case_idx: 9,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Bpchar(v) => Value::Variant {
                case_idx: 10,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Timestamp(v) => Value::Variant {
                case_idx: 11,
                case_value: Some(Box::new(v.to_string().into_value())),
            },
            DbValue::Timestamptz(v) => Value::Variant {
                case_idx: 12,
                case_value: Some(Box::new(v.to_string().into_value())),
            },
            DbValue::Date(v) => Value::Variant {
                case_idx: 13,
                case_value: Some(Box::new(v.to_string().into_value())),
            },
            DbValue::Time(v) => Value::Variant {
                case_idx: 14,
                case_value: Some(Box::new(v.to_string().into_value())),
            },
            DbValue::Timetz(v) => Value::Variant {
                case_idx: 15,
                case_value: Some(Box::new(v.to_string().into_value())),
            },
            DbValue::Interval(v) => Value::Variant {
                case_idx: 16,
                case_value: Some(Box::new(v.to_string().into_value())),
            },
            DbValue::Bytea(v) => Value::Variant {
                case_idx: 17,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Json(v) => Value::Variant {
                case_idx: 18,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Jsonb(v) => Value::Variant {
                case_idx: 19,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Jsonpath(v) => Value::Variant {
                case_idx: 20,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Xml(v) => Value::Variant {
                case_idx: 21,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Uuid(v) => Value::Variant {
                case_idx: 22,
                case_value: Some(Box::new(v.to_string().into_value())),
            },
            DbValue::Inet(v) => Value::Variant {
                case_idx: 23,
                case_value: Some(Box::new(v.to_string().into_value())),
            },
            DbValue::Cidr(v) => Value::Variant {
                case_idx: 24,
                case_value: Some(Box::new(v.to_string().into_value())),
            },
            DbValue::Macaddr(v) => Value::Variant {
                case_idx: 25,
                case_value: Some(Box::new(v.to_string().into_value())),
            },
            DbValue::Bit(v) => Value::Variant {
                case_idx: 26,
                case_value: Some(Box::new(v.iter().collect::<Vec<bool>>().into_value())),
            },
            DbValue::Varbit(v) => Value::Variant {
                case_idx: 27,
                case_value: Some(Box::new(v.iter().collect::<Vec<bool>>().into_value())),
            },
            DbValue::Int4range(v) => Value::Variant {
                case_idx: 28,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Int8range(v) => Value::Variant {
                case_idx: 29,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Numrange(v) => Value::Variant {
                case_idx: 30,
                case_value: Some(Box::new(v.map(|v| v.to_string()).into_value())),
            },
            DbValue::Tsrange(v) => Value::Variant {
                case_idx: 31,
                case_value: Some(Box::new(v.map(|v| v.to_string()).into_value())),
            },
            DbValue::Tstzrange(v) => Value::Variant {
                case_idx: 32,
                case_value: Some(Box::new(v.map(|v| v.to_string()).into_value())),
            },
            DbValue::Daterange(v) => Value::Variant {
                case_idx: 33,
                case_value: Some(Box::new(v.map(|v| v.to_string()).into_value())),
            },
            DbValue::Money(v) => Value::Variant {
                case_idx: 34,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Oid(v) => Value::Variant {
                case_idx: 35,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Enumeration(v) => Value::Variant {
                case_idx: 36,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Composite(v) => Value::Variant {
                case_idx: 37,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Domain(v) => Value::Variant {
                case_idx: 38,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Array(v) => Value::Variant {
                case_idx: 39,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Range(v) => Value::Variant {
                case_idx: 40,
                case_value: Some(Box::new(v.into_value())),
            },
            DbValue::Null => Value::Variant {
                case_idx: 41,
                case_value: None,
            },
        }
    }

    fn get_type() -> AnalysedType {
        fn get_tpe(root: bool) -> AnalysedType {
            let array_type = if root {
                analysed_type::case("array", get_tpe(false))
            } else {
                analysed_type::unit_case("array")
            };

            analysed_type::variant(vec![
                analysed_type::case("character", analysed_type::s8()),
                analysed_type::case("int2", analysed_type::s16()),
                analysed_type::case("int4", analysed_type::s32()),
                analysed_type::case("int8", analysed_type::s64()),
                analysed_type::case("float4", analysed_type::f32()),
                analysed_type::case("float8", analysed_type::f64()),
                analysed_type::case("numeric", analysed_type::str()),
                analysed_type::case("boolean", analysed_type::bool()),
                analysed_type::case("text", analysed_type::str()),
                analysed_type::case("varchar", analysed_type::str()),
                analysed_type::case("bpchar", analysed_type::str()),
                analysed_type::case("timestamp", analysed_type::str()),
                analysed_type::case("timestamptz", analysed_type::str()),
                analysed_type::case("time", analysed_type::str()),
                analysed_type::case("timetz", analysed_type::str()),
                analysed_type::case("date", analysed_type::str()),
                analysed_type::case("interval", analysed_type::str()),
                analysed_type::case("bytea", analysed_type::list(analysed_type::u8())),
                analysed_type::case("json", analysed_type::str()),
                analysed_type::case("jsonb", analysed_type::str()),
                analysed_type::case("jsonpath", analysed_type::str()),
                analysed_type::case("xml", analysed_type::str()),
                analysed_type::case("uuid", analysed_type::str()),
                analysed_type::case("inet", analysed_type::str()),
                analysed_type::case("cidr", analysed_type::str()),
                analysed_type::case("macaddr", analysed_type::str()),
                analysed_type::case("bit", analysed_type::list(analysed_type::bool())),
                analysed_type::case("varbit", analysed_type::list(analysed_type::bool())),
                analysed_type::case("int4range", ValuesRange::<i32>::get_type()),
                analysed_type::case("int8range", ValuesRange::<i64>::get_type()),
                analysed_type::case("numrange", ValuesRange::<String>::get_type()),
                analysed_type::case("tsrange", ValuesRange::<String>::get_type()),
                analysed_type::case("tstzrange", ValuesRange::<String>::get_type()),
                analysed_type::case("daterange", ValuesRange::<String>::get_type()),
                analysed_type::case("money", analysed_type::s64()),
                analysed_type::case("oid", analysed_type::u32()),
                analysed_type::case("enumeration", Enumeration::get_type()),
                analysed_type::case("composite", Composite::get_type()),
                analysed_type::case("domain", Domain::get_type()),
                analysed_type::case("range", Range::get_type()),
                array_type,
                analysed_type::case("null", analysed_type::str()),
            ])
        }
        get_tpe(true)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Encode, Decode)]
pub struct DbColumn {
    pub ordinal: u64,
    pub name: String,
    pub db_type: DbColumnType,
    pub db_type_name: String,
}

impl IntoValue for DbColumn {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.ordinal.into_value(),
            self.name.into_value(),
            self.db_type.into_value(),
            self.db_type_name.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("ordinal", analysed_type::u64()),
            analysed_type::field("name", analysed_type::str()),
            analysed_type::field("db-type", DbColumnType::get_type()),
            analysed_type::field("db-type-name", analysed_type::str()),
        ])
    }
}
