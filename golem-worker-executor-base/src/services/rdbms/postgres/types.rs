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
pub struct EnumType {
    pub name: String,
}

impl EnumType {
    pub fn new(name: String) -> Self {
        EnumType { name }
    }
}

impl NamedType for EnumType {
    fn name(&self) -> String {
        self.name.clone()
    }
}

impl Display for EnumType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
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

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct Enum {
    pub name: String,
    pub value: String,
}

impl Enum {
    pub fn new(name: String, value: String) -> Self {
        Enum { name, value }
    }
}

impl NamedType for Enum {
    fn name(&self) -> String {
        self.name.clone()
    }
}

impl Display for Enum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", self.name, self.value)
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
    Enum(EnumType),
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
            DbColumnType::Character => write!(f, "char"),
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
            DbColumnType::Oid => write!(f, "oid"),
            DbColumnType::Enum(v) => write!(f, "enum: {}", v),
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
            DbColumnType::Money => write!(f, "money"),
            DbColumnType::Null => write!(f, "null"),
        }
    }
}

impl IntoValue for DbColumnType {
    fn into_value(self) -> Value {
        todo!()
    }

    fn get_type() -> AnalysedType {
        todo!()
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
    Enum(Enum),
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
            DbValue::Enum(v) => write!(f, "{}", v),
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

    pub(crate) fn get_type(&self) -> DbColumnType {
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
            DbValue::Enum(v) => DbColumnType::Enum(EnumType::new(v.name.clone())),
            DbValue::Composite(v) => {
                DbColumnType::Composite(CompositeType::new(v.name.clone(), vec![]))
            }
            DbValue::Domain(v) => {
                let t = v.value.get_type();
                DbColumnType::Domain(DomainType::new(v.name.clone(), t))
            }
            DbValue::Range(r) => {
                let v = (*r.value).start_value().or((*r.value).end_value());
                let t = v.map(|v| v.get_type()).unwrap_or(DbColumnType::Null);
                DbColumnType::Range(RangeType::new(r.name.clone(), t))
            }
            DbValue::Array(vs) => {
                let t = if !vs.is_empty() {
                    let t = vs[0].get_type();
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
                            let t = v.map(|v| v.get_type()).unwrap_or(DbColumnType::Null);
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
        todo!()
    }

    fn get_type() -> AnalysedType {
        todo!()
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
