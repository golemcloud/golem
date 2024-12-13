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
use itertools::Itertools;
use sqlx::types::mac_address::MacAddress;
use sqlx::types::BitVec;
use std::fmt::{Debug, Display};
use std::net::IpAddr;
use std::ops::Bound;
use uuid::Uuid;

pub trait NamedType {
    fn name(&self) -> String;
}

#[derive(Clone, Debug, Eq, PartialEq)]
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

#[derive(Clone, Debug, Eq, PartialEq)]
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

#[derive(Clone, Debug, Eq, PartialEq)]
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

#[derive(Clone, Debug, PartialEq)]
pub struct Range<T> {
    pub start: Bound<T>,
    pub end: Bound<T>,
}

impl<T> Range<T> {
    pub fn new(start: Bound<T>, end: Bound<T>) -> Self {
        Range { start, end }
    }
}

impl<T: Debug> Display for Range<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} {:?}", self.start, self.end)
    }
}

#[derive(Clone, Debug, PartialEq)]
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
        write!(f, "{}m {}d {}ms", self.months, self.days, self.microseconds)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimeTz {
    pub time: chrono::NaiveTime,
    pub offset: chrono::FixedOffset,
}

impl TimeTz {
    pub fn new(time: chrono::NaiveTime, offset: chrono::FixedOffset) -> Self {
        TimeTz { time, offset }
    }
}

impl Display for TimeTz {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.time, self.offset)
    }
}

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DbColumnTypePrimitive {
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
    Enum(EnumType),
    Composite(CompositeType),
    Domain(DomainType),
    Oid,
}

impl Display for DbColumnTypePrimitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbColumnTypePrimitive::Character => write!(f, "char"),
            DbColumnTypePrimitive::Int2 => write!(f, "int2"),
            DbColumnTypePrimitive::Int4 => write!(f, "int4"),
            DbColumnTypePrimitive::Int8 => write!(f, "int8"),
            DbColumnTypePrimitive::Float4 => write!(f, "float4"),
            DbColumnTypePrimitive::Float8 => write!(f, "float8"),
            DbColumnTypePrimitive::Numeric => write!(f, "numeric"),
            DbColumnTypePrimitive::Boolean => write!(f, "boolean"),
            DbColumnTypePrimitive::Timestamp => write!(f, "timestamp"),
            DbColumnTypePrimitive::Date => write!(f, "date"),
            DbColumnTypePrimitive::Time => write!(f, "time"),
            DbColumnTypePrimitive::Timestamptz => write!(f, "timestamptz"),
            DbColumnTypePrimitive::Timetz => write!(f, "timetz"),
            DbColumnTypePrimitive::Interval => write!(f, "interval"),
            DbColumnTypePrimitive::Text => write!(f, "text"),
            DbColumnTypePrimitive::Varchar => write!(f, "varchar"),
            DbColumnTypePrimitive::Bpchar => write!(f, "bpchar"),
            DbColumnTypePrimitive::Bytea => write!(f, "bytea"),
            DbColumnTypePrimitive::Json => write!(f, "json"),
            DbColumnTypePrimitive::Jsonb => write!(f, "jsonb"),
            DbColumnTypePrimitive::Jsonpath => write!(f, "jsonpath"),
            DbColumnTypePrimitive::Xml => write!(f, "xml"),
            DbColumnTypePrimitive::Uuid => write!(f, "uuid"),
            DbColumnTypePrimitive::Inet => write!(f, "inet"),
            DbColumnTypePrimitive::Cidr => write!(f, "cidr"),
            DbColumnTypePrimitive::Macaddr => write!(f, "macaddr"),
            DbColumnTypePrimitive::Bit => write!(f, "bit"),
            DbColumnTypePrimitive::Varbit => write!(f, "varbit"),
            DbColumnTypePrimitive::Int4range => write!(f, "int4range"),
            DbColumnTypePrimitive::Int8range => write!(f, "int8range"),
            DbColumnTypePrimitive::Numrange => write!(f, "numrange"),
            DbColumnTypePrimitive::Tsrange => write!(f, "tsrange"),
            DbColumnTypePrimitive::Tstzrange => write!(f, "tstzrange"),
            DbColumnTypePrimitive::Daterange => write!(f, "daterange"),
            DbColumnTypePrimitive::Oid => write!(f, "oid"),
            DbColumnTypePrimitive::Enum(v) => write!(f, "enum: {}", v),
            DbColumnTypePrimitive::Composite(v) => {
                write!(f, "composite: {}", v)
            }
            DbColumnTypePrimitive::Domain(v) => {
                write!(f, "domain: {}", v)
            }
            DbColumnTypePrimitive::Money => write!(f, "money"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DbColumnType {
    Primitive(DbColumnTypePrimitive),
    Array(DbColumnTypePrimitive),
}

impl DbColumnType {
    pub(crate) fn into_array(self) -> DbColumnType {
        if let DbColumnType::Primitive(v) = self {
            DbColumnType::Array(v)
        } else {
            self
        }
    }
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
    Character(i8),
    Int2(i16),
    Int4(i32),
    Int8(i64),
    Float4(f32),
    Float8(f64),
    Numeric(BigDecimal),
    Boolean(bool),
    Timestamp(chrono::NaiveDateTime),
    Timestamptz(chrono::DateTime<chrono::Utc>),
    Date(chrono::NaiveDate),
    Time(chrono::NaiveTime),
    Timetz(TimeTz),
    Interval(Interval),
    Text(String),
    Varchar(String),
    Bpchar(String),
    Bytea(Vec<u8>),
    Json(serde_json::Value),
    Jsonb(serde_json::Value),
    Jsonpath(String),
    Xml(String),
    Uuid(Uuid),
    Inet(IpAddr),
    Cidr(IpAddr),
    Macaddr(MacAddress),
    Bit(BitVec),
    Varbit(BitVec),
    Int4range(Range<i32>),
    Int8range(Range<i64>),
    Numrange(Range<BigDecimal>),
    Tsrange(Range<chrono::NaiveDateTime>),
    Tstzrange(Range<chrono::DateTime<chrono::Utc>>),
    Daterange(Range<chrono::NaiveDate>),
    Money(i64),
    Enum(Enum),
    Composite(Composite),
    Domain(Domain),
    Oid(u32),
    Null,
}

impl Display for DbValuePrimitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbValuePrimitive::Character(v) => write!(f, "{}", v),
            DbValuePrimitive::Int2(v) => write!(f, "{}", v),
            DbValuePrimitive::Int4(v) => write!(f, "{}", v),
            DbValuePrimitive::Int8(v) => write!(f, "{}", v),
            DbValuePrimitive::Float4(v) => write!(f, "{}", v),
            DbValuePrimitive::Float8(v) => write!(f, "{}", v),
            DbValuePrimitive::Numeric(v) => write!(f, "{}", v),
            DbValuePrimitive::Boolean(v) => write!(f, "{}", v),
            DbValuePrimitive::Timestamp(v) => write!(f, "{}", v),
            DbValuePrimitive::Timestamptz(v) => write!(f, "{}", v),
            DbValuePrimitive::Date(v) => write!(f, "{}", v),
            DbValuePrimitive::Time(v) => write!(f, "{}", v),
            DbValuePrimitive::Timetz(v) => write!(f, "{}", v),
            DbValuePrimitive::Interval(v) => write!(f, "{}", v),
            DbValuePrimitive::Text(v) => write!(f, "{}", v),
            DbValuePrimitive::Varchar(v) => write!(f, "{}", v),
            DbValuePrimitive::Bpchar(v) => write!(f, "{}", v),
            DbValuePrimitive::Bytea(v) => write!(f, "{:?}", v),
            DbValuePrimitive::Json(v) => write!(f, "{}", v),
            DbValuePrimitive::Jsonb(v) => write!(f, "{}", v),
            DbValuePrimitive::Jsonpath(v) => write!(f, "{}", v),
            DbValuePrimitive::Xml(v) => write!(f, "{}", v),
            DbValuePrimitive::Uuid(v) => write!(f, "{}", v),
            DbValuePrimitive::Inet(v) => write!(f, "{}", v),
            DbValuePrimitive::Cidr(v) => write!(f, "{}", v),
            DbValuePrimitive::Macaddr(v) => write!(f, "{}", v),
            DbValuePrimitive::Bit(v) => write!(f, "{:?}", v),
            DbValuePrimitive::Varbit(v) => write!(f, "{:?}", v),
            DbValuePrimitive::Int4range(v) => write!(f, "{}", v),
            DbValuePrimitive::Int8range(v) => write!(f, "{}", v),
            DbValuePrimitive::Numrange(v) => write!(f, "{}", v),
            DbValuePrimitive::Tsrange(v) => write!(f, "{}", v),
            DbValuePrimitive::Tstzrange(v) => write!(f, "{}", v),
            DbValuePrimitive::Daterange(v) => write!(f, "{}", v),
            DbValuePrimitive::Oid(v) => write!(f, "{}", v),
            DbValuePrimitive::Money(v) => write!(f, "{}", v),
            DbValuePrimitive::Enum(v) => write!(f, "{}", v),
            DbValuePrimitive::Composite(v) => write!(f, "{}", v),
            DbValuePrimitive::Domain(v) => write!(f, "{}", v),
            DbValuePrimitive::Null => write!(f, "NULL"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DbValue {
    Primitive(DbValuePrimitive),
    Array(Vec<DbValuePrimitive>),
}

impl DbValue {
    pub(crate) fn array_from<T>(value: Option<Vec<T>>, f: impl Fn(T) -> DbValuePrimitive) -> Self {
        match value {
            Some(v) => DbValue::Array(v.into_iter().map(f).collect()),
            None => DbValue::Array(vec![]),
        }
    }

    pub(crate) fn primitive_from(value: Option<DbValuePrimitive>) -> Self {
        match value {
            Some(v) => DbValue::Primitive(v),
            None => DbValue::Primitive(DbValuePrimitive::Null),
        }
    }
}

impl Display for DbValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbValue::Primitive(v) => write!(f, "{}", v),
            DbValue::Array(v) => write!(f, "[{}]", v.iter().format(", ")),
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
