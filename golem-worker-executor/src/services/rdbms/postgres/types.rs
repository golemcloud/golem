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
    Enumeration, EnumerationType, Interval, SerializableComposite, SerializableCompositeType,
    SerializableDbColumn, SerializableDbColumnType, SerializableDbColumnTypeNode,
    SerializableDbValueNode, SerializableDomain, SerializableDomainType, SerializableRange,
    SerializableRangeType, SparseVec, TimeTz, ValuesRange,
};
use golem_wasm::NodeIndex;
use itertools::Itertools;
use mac_address::MacAddress;
use std::collections::Bound;
use std::fmt::{Debug, Display};
use std::net::IpAddr;
use uuid::Uuid;

pub trait NamedType {
    fn name(&self) -> String;
}

impl NamedType for EnumerationType {
    fn name(&self) -> String {
        self.name.clone()
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
    pub base_type: DbColumnType,
}

impl DomainType {
    pub fn new(name: String, base_type: DbColumnType) -> Self {
        DomainType { name, base_type }
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

#[derive(Clone, Debug, Eq, PartialEq)]
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

impl NamedType for Enumeration {
    fn name(&self) -> String {
        self.name.clone()
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
            self.values.iter().map(|v| format!("{v}")).format(", ")
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
    pub value: DbValue,
}

impl Domain {
    pub fn new(name: String, value: DbValue) -> Self {
        Domain { name, value }
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

#[derive(Clone, Debug, PartialEq)]
pub struct Range {
    pub name: String,
    pub value: ValuesRange<DbValue>,
}

impl Range {
    pub fn new(name: String, value: ValuesRange<DbValue>) -> Self {
        Range { name, value }
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

#[derive(Clone, Debug, Eq, PartialEq)]
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
    Domain(Box<DomainType>),
    Array(Box<DbColumnType>),
    Range(RangeType),
    Null,
    Vector,
    Halfvec,
    Sparsevec,
}

impl DbColumnType {
    pub fn into_array(self) -> DbColumnType {
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
            DbColumnType::Enumeration(v) => write!(f, "enumeration: {v}"),
            DbColumnType::Composite(v) => {
                write!(f, "composite: {v}")
            }
            DbColumnType::Domain(v) => {
                write!(f, "domain: {v}")
            }
            DbColumnType::Array(v) => {
                write!(f, "{v}[]")
            }
            DbColumnType::Range(v) => {
                write!(f, "range: {v}")
            }
            DbColumnType::Null => write!(f, "null"),
            DbColumnType::Vector => write!(f, "vector"),
            DbColumnType::Halfvec => write!(f, "halfvec"),
            DbColumnType::Sparsevec => write!(f, "sparsevec"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DbValue {
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
    Json(String),
    Jsonb(String),
    Jsonpath(String),
    Xml(String),
    Uuid(Uuid),
    Inet(IpAddr),
    Cidr(IpAddr),
    Macaddr(MacAddress),
    Bit(BitVec),
    Varbit(BitVec),
    Int4range(ValuesRange<i32>),
    Int8range(ValuesRange<i64>),
    Numrange(ValuesRange<BigDecimal>),
    Tsrange(ValuesRange<chrono::NaiveDateTime>),
    Tstzrange(ValuesRange<chrono::DateTime<chrono::Utc>>),
    Daterange(ValuesRange<chrono::NaiveDate>),
    Money(i64),
    Oid(u32),
    Enumeration(Enumeration),
    Composite(Composite),
    Domain(Box<Domain>),
    Array(Vec<DbValue>),
    Range(Box<Range>),
    Null,
    Vector(Vec<f32>),
    Halfvec(Vec<half::f16>),
    Sparsevec(SparseVec),
}

impl From<DbValue> for SerializableDbValue {
    fn from(value: DbValue) -> Self {
        fn add_node(target: &mut SerializableDbValue, value: SerializableDbValueNode) -> NodeIndex {
            target.nodes.push(value);
            (target.nodes.len() - 1) as NodeIndex
        }

        fn add_db_value(target: &mut SerializableDbValue, value: DbValue) -> NodeIndex {
            match value {
                DbValue::Character(v) => add_node(target, SerializableDbValueNode::Tinyint(v)),
                DbValue::Int2(v) => add_node(target, SerializableDbValueNode::Smallint(v)),
                DbValue::Int4(v) => add_node(target, SerializableDbValueNode::Int(v)),
                DbValue::Int8(v) => add_node(target, SerializableDbValueNode::Bigint(v)),
                DbValue::Float4(v) => add_node(target, SerializableDbValueNode::Float(v)),
                DbValue::Float8(v) => add_node(target, SerializableDbValueNode::Double(v)),
                DbValue::Numeric(v) => add_node(target, SerializableDbValueNode::Decimal(v)),
                DbValue::Boolean(v) => add_node(target, SerializableDbValueNode::Boolean(v)),
                DbValue::Timestamp(v) => add_node(target, SerializableDbValueNode::Timestamp(v)),
                DbValue::Timestamptz(v) => {
                    add_node(target, SerializableDbValueNode::Timestamptz(v))
                }
                DbValue::Date(v) => add_node(target, SerializableDbValueNode::Date(v)),
                DbValue::Time(v) => add_node(target, SerializableDbValueNode::Time(v)),
                DbValue::Timetz(v) => add_node(target, SerializableDbValueNode::Timetz(v)),
                DbValue::Interval(v) => add_node(target, SerializableDbValueNode::Interval(v)),
                DbValue::Text(v) => add_node(target, SerializableDbValueNode::Text(v)),
                DbValue::Varchar(v) => add_node(target, SerializableDbValueNode::Varchar(v)),
                DbValue::Bpchar(v) => add_node(target, SerializableDbValueNode::Bpchar(v)),
                DbValue::Bytea(v) => add_node(target, SerializableDbValueNode::Bytea(v)),
                DbValue::Json(v) => add_node(target, SerializableDbValueNode::Json(v)),
                DbValue::Jsonb(v) => add_node(target, SerializableDbValueNode::Jsonb(v)),
                DbValue::Jsonpath(v) => add_node(target, SerializableDbValueNode::Jsonpath(v)),
                DbValue::Xml(v) => add_node(target, SerializableDbValueNode::Xml(v)),
                DbValue::Uuid(v) => add_node(target, SerializableDbValueNode::Uuid(v)),
                DbValue::Inet(v) => add_node(target, SerializableDbValueNode::Inet(v.into())),
                DbValue::Cidr(v) => add_node(target, SerializableDbValueNode::Cidr(v.into())),
                DbValue::Macaddr(v) => add_node(target, SerializableDbValueNode::Macaddr(v.into())),
                DbValue::Bit(v) => add_node(target, SerializableDbValueNode::Bit(v)),
                DbValue::Varbit(v) => add_node(target, SerializableDbValueNode::Varbit(v)),
                DbValue::Int4range(v) => add_node(target, SerializableDbValueNode::Int4range(v)),
                DbValue::Int8range(v) => add_node(target, SerializableDbValueNode::Int8range(v)),
                DbValue::Numrange(v) => add_node(target, SerializableDbValueNode::Numrange(v)),
                DbValue::Tsrange(v) => add_node(target, SerializableDbValueNode::Tsrange(v)),
                DbValue::Tstzrange(v) => add_node(target, SerializableDbValueNode::Tstzrange(v)),
                DbValue::Daterange(v) => add_node(target, SerializableDbValueNode::Daterange(v)),
                DbValue::Money(v) => add_node(target, SerializableDbValueNode::Money(v)),
                DbValue::Oid(v) => add_node(target, SerializableDbValueNode::Oid(v)),
                DbValue::Enumeration(v) => {
                    add_node(target, SerializableDbValueNode::Enumeration(v))
                }
                DbValue::Composite(v) => {
                    let mut indices = Vec::with_capacity(v.values.len());
                    for value in v.values {
                        indices.push(add_db_value(target, value));
                    }
                    add_node(
                        target,
                        SerializableDbValueNode::Composite(SerializableComposite {
                            name: v.name,
                            values: indices,
                        }),
                    )
                }
                DbValue::Domain(v) => {
                    let index = add_db_value(target, v.value);
                    add_node(
                        target,
                        SerializableDbValueNode::Domain(SerializableDomain {
                            name: v.name,
                            value: index,
                        }),
                    )
                }
                DbValue::Array(v) => {
                    let mut indices = Vec::with_capacity(v.len());
                    for value in v {
                        indices.push(add_db_value(target, value));
                    }
                    add_node(target, SerializableDbValueNode::Array(indices))
                }
                DbValue::Range(v) => {
                    let start = v.value.start.map(|x| add_db_value(target, x));
                    let end = v.value.end.map(|x| add_db_value(target, x));
                    add_node(
                        target,
                        SerializableDbValueNode::Range(SerializableRange {
                            name: v.name,
                            value: ValuesRange::new(start, end),
                        }),
                    )
                }
                DbValue::Null => add_node(target, SerializableDbValueNode::Null),
                DbValue::Vector(v) => add_node(target, SerializableDbValueNode::Vector(v)),
                DbValue::Halfvec(v) => add_node(
                    target,
                    SerializableDbValueNode::Halfvec(v.into_iter().map(|v| v.to_f32()).collect()),
                ),
                DbValue::Sparsevec(v) => add_node(target, SerializableDbValueNode::Sparsevec(v)),
            }
        }

        let mut result = SerializableDbValue { nodes: vec![] };
        add_db_value(&mut result, value);
        result
    }
}

impl TryFrom<SerializableDbValue> for DbValue {
    type Error = String;

    fn try_from(value: SerializableDbValue) -> Result<Self, Self::Error> {
        fn convert_node(
            node: SerializableDbValueNode,
            nodes: &mut Vec<Option<SerializableDbValueNode>>,
        ) -> Result<DbValue, String> {
            match node {
                SerializableDbValueNode::Tinyint(v) => Ok(DbValue::Character(v)),
                SerializableDbValueNode::Smallint(v) => Ok(DbValue::Int2(v)),
                SerializableDbValueNode::Int(v) => Ok(DbValue::Int4(v)),
                SerializableDbValueNode::Bigint(v) => Ok(DbValue::Int8(v)),
                SerializableDbValueNode::Float(v) => Ok(DbValue::Float4(v)),
                SerializableDbValueNode::Double(v) => Ok(DbValue::Float8(v)),
                SerializableDbValueNode::Decimal(v) => Ok(DbValue::Numeric(v)),
                SerializableDbValueNode::Boolean(v) => Ok(DbValue::Boolean(v)),
                SerializableDbValueNode::Timestamp(v) => Ok(DbValue::Timestamp(v)),
                SerializableDbValueNode::Timestamptz(v) => Ok(DbValue::Timestamptz(v)),
                SerializableDbValueNode::Date(v) => Ok(DbValue::Date(v)),
                SerializableDbValueNode::Time(v) => Ok(DbValue::Time(v)),
                SerializableDbValueNode::Timetz(v) => Ok(DbValue::Timetz(v)),
                SerializableDbValueNode::Interval(v) => Ok(DbValue::Interval(v)),
                SerializableDbValueNode::Text(v) => Ok(DbValue::Text(v)),
                SerializableDbValueNode::Varchar(v) => Ok(DbValue::Varchar(v)),
                SerializableDbValueNode::Bpchar(v) => Ok(DbValue::Bpchar(v)),
                SerializableDbValueNode::Bytea(v) => Ok(DbValue::Bytea(v)),
                SerializableDbValueNode::Json(v) => Ok(DbValue::Json(v)),
                SerializableDbValueNode::Jsonb(v) => Ok(DbValue::Jsonb(v)),
                SerializableDbValueNode::Jsonpath(v) => Ok(DbValue::Jsonpath(v)),
                SerializableDbValueNode::Xml(v) => Ok(DbValue::Xml(v)),
                SerializableDbValueNode::Uuid(v) => Ok(DbValue::Uuid(v)),
                SerializableDbValueNode::Inet(v) => Ok(DbValue::Inet(v.into())),
                SerializableDbValueNode::Cidr(v) => Ok(DbValue::Cidr(v.into())),
                SerializableDbValueNode::Macaddr(v) => Ok(DbValue::Macaddr(v.into())),
                SerializableDbValueNode::Bit(v) => Ok(DbValue::Bit(v)),
                SerializableDbValueNode::Varbit(v) => Ok(DbValue::Varbit(v)),
                SerializableDbValueNode::Int4range(v) => Ok(DbValue::Int4range(v)),
                SerializableDbValueNode::Int8range(v) => Ok(DbValue::Int8range(v)),
                SerializableDbValueNode::Numrange(v) => Ok(DbValue::Numrange(v)),
                SerializableDbValueNode::Tsrange(v) => Ok(DbValue::Tsrange(v)),
                SerializableDbValueNode::Tstzrange(v) => Ok(DbValue::Tstzrange(v)),
                SerializableDbValueNode::Daterange(v) => Ok(DbValue::Daterange(v)),
                SerializableDbValueNode::Money(v) => Ok(DbValue::Money(v)),
                SerializableDbValueNode::Oid(v) => Ok(DbValue::Oid(v)),
                SerializableDbValueNode::Enumeration(v) => Ok(DbValue::Enumeration(v)),
                SerializableDbValueNode::Composite(v) => {
                    let mut values = Vec::with_capacity(v.values.len());
                    for index in v.values {
                        let node = nodes[index as usize].take().unwrap();
                        values.push(convert_node(node, nodes)?);
                    }
                    Ok(DbValue::Composite(Composite {
                        name: v.name,
                        values,
                    }))
                }
                SerializableDbValueNode::Domain(v) => {
                    let node = nodes[v.value as usize].take().unwrap();
                    let value = convert_node(node, nodes)?;
                    Ok(DbValue::Domain(Box::new(Domain {
                        name: v.name,
                        value,
                    })))
                }
                SerializableDbValueNode::Array(v) => {
                    let mut values = Vec::with_capacity(v.len());
                    for index in v {
                        let node = nodes[index as usize].take().unwrap();
                        values.push(convert_node(node, nodes)?);
                    }
                    Ok(DbValue::Array(values))
                }
                SerializableDbValueNode::Range(v) => {
                    let start = transpose_bound(
                        v.value
                            .start
                            .map(|x| convert_node(nodes[x as usize].take().unwrap(), nodes)),
                    )?;
                    let end = transpose_bound(
                        v.value
                            .end
                            .map(|x| convert_node(nodes[x as usize].take().unwrap(), nodes)),
                    )?;
                    Ok(DbValue::Range(Box::new(Range {
                        name: v.name,
                        value: ValuesRange::new(start, end),
                    })))
                }
                SerializableDbValueNode::Null => Ok(DbValue::Null),
                SerializableDbValueNode::Vector(v) => Ok(DbValue::Vector(v)),
                SerializableDbValueNode::Halfvec(v) => Ok(DbValue::Halfvec(
                    half::vec::HalfFloatVecExt::from_f32_slice(&v),
                )),
                SerializableDbValueNode::Sparsevec(v) => Ok(DbValue::Sparsevec(v)),
                _ => Err(format!(
                    "Unsupported SerializableDbValueNode variant for PostgreSQL: {:?}",
                    node
                )),
            }
        }

        if value.nodes.is_empty() {
            return Err("Empty SerializableDbValue".to_string());
        }

        let len = value.nodes.len() - 1;
        let mut nodes = value.nodes.into_iter().map(Some).collect::<Vec<_>>();
        let last = nodes[len].take().unwrap();
        convert_node(last, &mut nodes)
    }
}

fn transpose_bound<T, E>(bound: Bound<Result<T, E>>) -> Result<Bound<T>, E> {
    match bound {
        Bound::Unbounded => Ok(Bound::Unbounded),
        Bound::Included(v) => v.map(|v| Bound::Included(v)),
        Bound::Excluded(v) => v.map(|v| Bound::Excluded(v)),
    }
}

impl Display for DbValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbValue::Character(v) => write!(f, "{v}"),
            DbValue::Int2(v) => write!(f, "{v}"),
            DbValue::Int4(v) => write!(f, "{v}"),
            DbValue::Int8(v) => write!(f, "{v}"),
            DbValue::Float4(v) => write!(f, "{v}"),
            DbValue::Float8(v) => write!(f, "{v}"),
            DbValue::Numeric(v) => write!(f, "{v}"),
            DbValue::Boolean(v) => write!(f, "{v}"),
            DbValue::Timestamp(v) => write!(f, "{v}"),
            DbValue::Timestamptz(v) => write!(f, "{v}"),
            DbValue::Date(v) => write!(f, "{v}"),
            DbValue::Time(v) => write!(f, "{v}"),
            DbValue::Timetz(v) => write!(f, "{v}"),
            DbValue::Interval(v) => write!(f, "{v}"),
            DbValue::Text(v) => write!(f, "{v}"),
            DbValue::Varchar(v) => write!(f, "{v}"),
            DbValue::Bpchar(v) => write!(f, "{v}"),
            DbValue::Bytea(v) => write!(f, "{v:?}"),
            DbValue::Json(v) => write!(f, "{v}"),
            DbValue::Jsonb(v) => write!(f, "{v}"),
            DbValue::Jsonpath(v) => write!(f, "{v}"),
            DbValue::Xml(v) => write!(f, "{v}"),
            DbValue::Uuid(v) => write!(f, "{v}"),
            DbValue::Inet(v) => write!(f, "{v}"),
            DbValue::Cidr(v) => write!(f, "{v}"),
            DbValue::Macaddr(v) => write!(f, "{v}"),
            DbValue::Bit(v) => write!(f, "{v:?}"),
            DbValue::Varbit(v) => write!(f, "{v:?}"),
            DbValue::Int4range(v) => write!(f, "{v}"),
            DbValue::Int8range(v) => write!(f, "{v}"),
            DbValue::Numrange(v) => write!(f, "{v}"),
            DbValue::Tsrange(v) => write!(f, "{v}"),
            DbValue::Tstzrange(v) => write!(f, "{v}"),
            DbValue::Daterange(v) => write!(f, "{v}"),
            DbValue::Oid(v) => write!(f, "{v}"),
            DbValue::Money(v) => write!(f, "{v}"),
            DbValue::Enumeration(v) => write!(f, "{v}"),
            DbValue::Composite(v) => write!(f, "{v}"),
            DbValue::Domain(v) => write!(f, "{v}"),
            DbValue::Array(v) => write!(f, "[{}]", v.iter().format(", ")),
            DbValue::Range(v) => write!(f, "{v}"),
            DbValue::Null => write!(f, "NULL"),
            DbValue::Vector(v) => write!(f, "[{}]", v.iter().format(", ")),
            DbValue::Halfvec(v) => write!(f, "[{}]", v.iter().format(", ")),
            DbValue::Sparsevec(v) => write!(f, "{v}"),
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

    pub fn get_column_type(&self) -> DbColumnType {
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
                DbColumnType::Domain(Box::new(DomainType::new(v.name.clone(), t)))
            }
            DbValue::Range(r) => {
                let v = r.value.start_value().or(r.value.end_value());
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
                                        v.value.start_value().or(v.value.end_value())
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
            DbValue::Vector(_) => DbColumnType::Vector,
            DbValue::Halfvec(_) => DbColumnType::Halfvec,
            DbValue::Sparsevec(_) => DbColumnType::Sparsevec,
        }
    }
}

impl From<DbColumnType> for SerializableDbColumnType {
    fn from(value: DbColumnType) -> Self {
        fn build_serializable_db_column_type(
            db_type: DbColumnType,
            nodes: &mut Vec<SerializableDbColumnTypeNode>,
        ) -> NodeIndex {
            match db_type {
                DbColumnType::Character => {
                    nodes.push(SerializableDbColumnTypeNode::Character);
                }
                DbColumnType::Int2 => {
                    nodes.push(SerializableDbColumnTypeNode::Smallint);
                }
                DbColumnType::Int4 => {
                    nodes.push(SerializableDbColumnTypeNode::Int);
                }
                DbColumnType::Int8 => {
                    nodes.push(SerializableDbColumnTypeNode::Bigint);
                }
                DbColumnType::Float4 => {
                    nodes.push(SerializableDbColumnTypeNode::Float);
                }
                DbColumnType::Float8 => {
                    nodes.push(SerializableDbColumnTypeNode::Double);
                }
                DbColumnType::Numeric => {
                    nodes.push(SerializableDbColumnTypeNode::Decimal);
                }
                DbColumnType::Boolean => {
                    nodes.push(SerializableDbColumnTypeNode::Boolean);
                }
                DbColumnType::Text => {
                    nodes.push(SerializableDbColumnTypeNode::Text);
                }
                DbColumnType::Varchar => {
                    nodes.push(SerializableDbColumnTypeNode::Varchar);
                }
                DbColumnType::Bpchar => {
                    nodes.push(SerializableDbColumnTypeNode::Bpchar);
                }
                DbColumnType::Timestamp => {
                    nodes.push(SerializableDbColumnTypeNode::Timestamp);
                }
                DbColumnType::Timestamptz => {
                    nodes.push(SerializableDbColumnTypeNode::Timestamptz);
                }
                DbColumnType::Date => {
                    nodes.push(SerializableDbColumnTypeNode::Date);
                }
                DbColumnType::Time => {
                    nodes.push(SerializableDbColumnTypeNode::Time);
                }
                DbColumnType::Timetz => {
                    nodes.push(SerializableDbColumnTypeNode::Timetz);
                }
                DbColumnType::Interval => {
                    nodes.push(SerializableDbColumnTypeNode::Interval);
                }
                DbColumnType::Bytea => {
                    nodes.push(SerializableDbColumnTypeNode::Bytea);
                }
                DbColumnType::Uuid => {
                    nodes.push(SerializableDbColumnTypeNode::Uuid);
                }
                DbColumnType::Xml => {
                    nodes.push(SerializableDbColumnTypeNode::Xml);
                }
                DbColumnType::Json => {
                    nodes.push(SerializableDbColumnTypeNode::Json);
                }
                DbColumnType::Jsonb => {
                    nodes.push(SerializableDbColumnTypeNode::Jsonb);
                }
                DbColumnType::Jsonpath => {
                    nodes.push(SerializableDbColumnTypeNode::Jsonpath);
                }
                DbColumnType::Inet => {
                    nodes.push(SerializableDbColumnTypeNode::Inet);
                }
                DbColumnType::Cidr => {
                    nodes.push(SerializableDbColumnTypeNode::Cidr);
                }
                DbColumnType::Macaddr => {
                    nodes.push(SerializableDbColumnTypeNode::Macaddr);
                }
                DbColumnType::Bit => {
                    nodes.push(SerializableDbColumnTypeNode::Bit);
                }
                DbColumnType::Varbit => {
                    nodes.push(SerializableDbColumnTypeNode::Varbit);
                }
                DbColumnType::Int4range => {
                    nodes.push(SerializableDbColumnTypeNode::Int4range);
                }
                DbColumnType::Int8range => {
                    nodes.push(SerializableDbColumnTypeNode::Int8range);
                }
                DbColumnType::Numrange => {
                    nodes.push(SerializableDbColumnTypeNode::Numrange);
                }
                DbColumnType::Tsrange => {
                    nodes.push(SerializableDbColumnTypeNode::Tsrange);
                }
                DbColumnType::Tstzrange => {
                    nodes.push(SerializableDbColumnTypeNode::Tstzrange);
                }
                DbColumnType::Daterange => {
                    nodes.push(SerializableDbColumnTypeNode::Daterange);
                }
                DbColumnType::Money => {
                    nodes.push(SerializableDbColumnTypeNode::Money);
                }
                DbColumnType::Oid => {
                    nodes.push(SerializableDbColumnTypeNode::Oid);
                }
                DbColumnType::Enumeration(enum_type) => {
                    nodes.push(SerializableDbColumnTypeNode::Enumeration(enum_type));
                }
                DbColumnType::Composite(composite_type) => {
                    let attributes: Vec<(String, NodeIndex)> = composite_type
                        .attributes
                        .into_iter()
                        .map(|(name, attr_type)| {
                            (
                                name.clone(),
                                build_serializable_db_column_type(attr_type, nodes),
                            )
                        })
                        .collect();
                    nodes.push(SerializableDbColumnTypeNode::Composite(
                        SerializableCompositeType {
                            name: composite_type.name.clone(),
                            attributes,
                        },
                    ));
                }
                DbColumnType::Domain(domain_type) => {
                    let node = SerializableDbColumnTypeNode::Domain(SerializableDomainType {
                        name: domain_type.name,
                        base_type: build_serializable_db_column_type(domain_type.base_type, nodes),
                    });
                    nodes.push(node);
                }
                DbColumnType::Array(element_type) => {
                    let node = SerializableDbColumnTypeNode::Array(
                        build_serializable_db_column_type(*element_type, nodes),
                    );
                    nodes.push(node);
                }
                DbColumnType::Range(range_type) => {
                    let node = SerializableDbColumnTypeNode::Range(SerializableRangeType {
                        name: range_type.name,
                        base_type: build_serializable_db_column_type(*range_type.base_type, nodes),
                    });
                    nodes.push(node);
                }
                DbColumnType::Null => {
                    nodes.push(SerializableDbColumnTypeNode::Null);
                }
                DbColumnType::Vector => {
                    nodes.push(SerializableDbColumnTypeNode::Vector);
                }
                DbColumnType::Halfvec => {
                    nodes.push(SerializableDbColumnTypeNode::Halfvec);
                }
                DbColumnType::Sparsevec => {
                    nodes.push(SerializableDbColumnTypeNode::Sparsevec);
                }
            }

            (nodes.len() - 1) as NodeIndex
        }

        let mut nodes = Vec::new();
        build_serializable_db_column_type(value, &mut nodes);
        SerializableDbColumnType { nodes }
    }
}

impl TryFrom<SerializableDbColumnType> for DbColumnType {
    type Error = String;

    fn try_from(value: SerializableDbColumnType) -> Result<Self, Self::Error> {
        fn resolve_db_column_type(
            nodes: &mut Vec<Option<SerializableDbColumnTypeNode>>,
            index: NodeIndex,
        ) -> Result<DbColumnType, String> {
            let node = nodes[index as usize].take().unwrap();
            match node {
                SerializableDbColumnTypeNode::Character => Ok(DbColumnType::Character),
                SerializableDbColumnTypeNode::Smallint => Ok(DbColumnType::Int2),
                SerializableDbColumnTypeNode::Int => Ok(DbColumnType::Int4),
                SerializableDbColumnTypeNode::Bigint => Ok(DbColumnType::Int8),
                SerializableDbColumnTypeNode::Float => Ok(DbColumnType::Float4),
                SerializableDbColumnTypeNode::Double => Ok(DbColumnType::Float8),
                SerializableDbColumnTypeNode::Decimal => Ok(DbColumnType::Numeric),
                SerializableDbColumnTypeNode::Boolean => Ok(DbColumnType::Boolean),
                SerializableDbColumnTypeNode::Text => Ok(DbColumnType::Text),
                SerializableDbColumnTypeNode::Varchar => Ok(DbColumnType::Varchar),
                SerializableDbColumnTypeNode::Bpchar => Ok(DbColumnType::Bpchar),
                SerializableDbColumnTypeNode::Timestamp => Ok(DbColumnType::Timestamp),
                SerializableDbColumnTypeNode::Timestamptz => Ok(DbColumnType::Timestamptz),
                SerializableDbColumnTypeNode::Date => Ok(DbColumnType::Date),
                SerializableDbColumnTypeNode::Time => Ok(DbColumnType::Time),
                SerializableDbColumnTypeNode::Timetz => Ok(DbColumnType::Timetz),
                SerializableDbColumnTypeNode::Interval => Ok(DbColumnType::Interval),
                SerializableDbColumnTypeNode::Bytea => Ok(DbColumnType::Bytea),
                SerializableDbColumnTypeNode::Uuid => Ok(DbColumnType::Uuid),
                SerializableDbColumnTypeNode::Xml => Ok(DbColumnType::Xml),
                SerializableDbColumnTypeNode::Json => Ok(DbColumnType::Json),
                SerializableDbColumnTypeNode::Jsonb => Ok(DbColumnType::Jsonb),
                SerializableDbColumnTypeNode::Jsonpath => Ok(DbColumnType::Jsonpath),
                SerializableDbColumnTypeNode::Inet => Ok(DbColumnType::Inet),
                SerializableDbColumnTypeNode::Cidr => Ok(DbColumnType::Cidr),
                SerializableDbColumnTypeNode::Macaddr => Ok(DbColumnType::Macaddr),
                SerializableDbColumnTypeNode::Bit => Ok(DbColumnType::Bit),
                SerializableDbColumnTypeNode::Varbit => Ok(DbColumnType::Varbit),
                SerializableDbColumnTypeNode::Int4range => Ok(DbColumnType::Int4range),
                SerializableDbColumnTypeNode::Int8range => Ok(DbColumnType::Int8range),
                SerializableDbColumnTypeNode::Numrange => Ok(DbColumnType::Numrange),
                SerializableDbColumnTypeNode::Tsrange => Ok(DbColumnType::Tsrange),
                SerializableDbColumnTypeNode::Tstzrange => Ok(DbColumnType::Tstzrange),
                SerializableDbColumnTypeNode::Daterange => Ok(DbColumnType::Daterange),
                SerializableDbColumnTypeNode::Money => Ok(DbColumnType::Money),
                SerializableDbColumnTypeNode::Oid => Ok(DbColumnType::Oid),
                SerializableDbColumnTypeNode::Enumeration(enum_type) => {
                    Ok(DbColumnType::Enumeration(enum_type.clone()))
                }
                SerializableDbColumnTypeNode::Composite(composite_type) => {
                    let attributes: Result<Vec<(String, DbColumnType)>, String> = composite_type
                        .attributes
                        .into_iter()
                        .map(|(name, attr_index)| {
                            let attr_type = resolve_db_column_type(nodes, attr_index)?;
                            Ok((name, attr_type))
                        })
                        .collect();
                    let attributes = attributes?;
                    Ok(DbColumnType::Composite(CompositeType::new(
                        composite_type.name,
                        attributes,
                    )))
                }
                SerializableDbColumnTypeNode::Domain(domain_type) => {
                    let base_type = resolve_db_column_type(nodes, domain_type.base_type)?;
                    Ok(DbColumnType::Domain(Box::new(DomainType::new(
                        domain_type.name,
                        base_type,
                    ))))
                }
                SerializableDbColumnTypeNode::Array(element_index) => {
                    let element_type = resolve_db_column_type(nodes, element_index)?;
                    Ok(DbColumnType::Array(Box::new(element_type)))
                }
                SerializableDbColumnTypeNode::Range(range_type) => {
                    let base_type = resolve_db_column_type(nodes, range_type.base_type)?;
                    Ok(DbColumnType::Range(RangeType::new(
                        range_type.name.clone(),
                        base_type,
                    )))
                }
                SerializableDbColumnTypeNode::Null => Ok(DbColumnType::Null),
                SerializableDbColumnTypeNode::Vector => Ok(DbColumnType::Vector),
                SerializableDbColumnTypeNode::Halfvec => Ok(DbColumnType::Halfvec),
                SerializableDbColumnTypeNode::Sparsevec => Ok(DbColumnType::Sparsevec),
                _ => Err(format!(
                    "Unsupported SerializableDbColumnTypeNode variant: {:?}",
                    node
                )),
            }
        }

        if value.nodes.is_empty() {
            return Err("SerializableDbColumnType must have at least one node".to_string());
        }

        let last_idx = (value.nodes.len() - 1) as NodeIndex;
        let mut nodes = value.nodes.into_iter().map(Some).collect::<Vec<_>>();
        resolve_db_column_type(&mut nodes, last_idx)
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
    use crate::services::rdbms::postgres::types as postgres_types;
    use bigdecimal::BigDecimal;
    use bit_vec::BitVec;
    use mac_address::MacAddress;
    use serde_json::json;
    use std::collections::Bound;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use uuid::Uuid;

    pub(crate) fn get_test_db_column_types() -> Vec<postgres_types::DbColumnType> {
        let mut values: Vec<postgres_types::DbColumnType> = vec![];

        let value = postgres_types::DbColumnType::Composite(postgres_types::CompositeType::new(
            "inventory_item".to_string(),
            vec![
                ("product_id".to_string(), postgres_types::DbColumnType::Uuid),
                ("name".to_string(), postgres_types::DbColumnType::Text),
                (
                    "supplier_id".to_string(),
                    postgres_types::DbColumnType::Int4,
                ),
                ("price".to_string(), postgres_types::DbColumnType::Numeric),
                (
                    "tags".to_string(),
                    postgres_types::DbColumnType::Text.into_array(),
                ),
                (
                    "interval".to_string(),
                    postgres_types::DbColumnType::Range(postgres_types::RangeType::new(
                        "float4range".to_string(),
                        postgres_types::DbColumnType::Float4,
                    )),
                ),
            ],
        ));

        values.push(value.clone());
        values.push(value.clone().into_array());

        let value =
            postgres_types::DbColumnType::Domain(Box::new(postgres_types::DomainType::new(
                "posint8".to_string(),
                postgres_types::DbColumnType::Int8,
            )));

        values.push(value.clone());
        values.push(value.clone().into_array());

        let value = postgres_types::DbColumnType::Range(postgres_types::RangeType::new(
            "float4range".to_string(),
            postgres_types::DbColumnType::Float4,
        ));

        values.push(value.clone());
        values.push(value.clone().into_array());

        let value = postgres_types::DbColumnType::Range(postgres_types::RangeType::new(
            "a_custom_type_range".to_string(),
            postgres_types::DbColumnType::Composite(postgres_types::CompositeType::new(
                "a_custom_type".to_string(),
                vec![("val".to_string(), postgres_types::DbColumnType::Int4)],
            )),
        ));

        values.push(value.clone());
        values.push(value.clone().into_array());

        values
    }

    pub(crate) fn get_test_db_values() -> Vec<postgres_types::DbValue> {
        let tstzbounds = postgres_types::ValuesRange::new(
            Bound::Included(chrono::DateTime::from_naive_utc_and_offset(
                chrono::NaiveDateTime::new(
                    chrono::NaiveDate::from_ymd_opt(2023, 3, 2).unwrap(),
                    chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                ),
                chrono::Utc,
            )),
            Bound::Excluded(chrono::DateTime::from_naive_utc_and_offset(
                chrono::NaiveDateTime::new(
                    chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
                    chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                ),
                chrono::Utc,
            )),
        );
        let tsbounds = postgres_types::ValuesRange::new(
            Bound::Included(chrono::NaiveDateTime::new(
                chrono::NaiveDate::from_ymd_opt(2022, 2, 2).unwrap(),
                chrono::NaiveTime::from_hms_opt(16, 50, 30).unwrap(),
            )),
            Bound::Excluded(chrono::NaiveDateTime::new(
                chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
                chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
            )),
        );

        vec![
            postgres_types::DbValue::Array(vec![
                postgres_types::DbValue::Enumeration(postgres_types::Enumeration::new(
                    "a_test_enum".to_string(),
                    "second".to_string(),
                )),
                postgres_types::DbValue::Enumeration(postgres_types::Enumeration::new(
                    "a_test_enum".to_string(),
                    "third".to_string(),
                )),
            ]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Character(2)]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Int2(1)]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Int4(2)]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Int8(3)]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Float4(4.0)]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Float8(5.0)]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Numeric(
                BigDecimal::from(48888),
            )]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Boolean(true)]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Text("text".to_string())]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Varchar(
                "varchar".to_string(),
            )]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Bpchar(
                "0123456789".to_string(),
            )]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Timestamp(
                chrono::NaiveDateTime::new(
                    chrono::NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
                    chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                ),
            )]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Timestamptz(
                chrono::DateTime::from_naive_utc_and_offset(
                    chrono::NaiveDateTime::new(
                        chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                        chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                    ),
                    chrono::Utc,
                ),
            )]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Date(
                chrono::NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
            )]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Time(
                chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
            )]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Timetz(
                postgres_types::TimeTz::new(
                    chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                    chrono::FixedOffset::east_opt(5 * 60 * 60).unwrap(),
                ),
            )]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Interval(
                postgres_types::Interval::new(10, 20, 30),
            )]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Bytea(
                "bytea".as_bytes().to_vec(),
            )]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Uuid(Uuid::new_v4())]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Json(
                json!(
                       {
                          "id": 2
                       }
                )
                .to_string(),
            )]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Jsonb(
                json!(
                       {
                          "index": 4
                       }
                )
                .to_string(),
            )]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Inet(IpAddr::V4(
                Ipv4Addr::new(127, 0, 0, 1),
            ))]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Cidr(IpAddr::V6(
                Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0xc00a, 0x2ff),
            ))]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Macaddr(
                MacAddress::new([0, 1, 2, 3, 4, 1]),
            )]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Bit(BitVec::from_iter(
                vec![true, false, true],
            ))]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Varbit(
                BitVec::from_iter(vec![true, false, false]),
            )]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Xml(
                "<foo>200</foo>".to_string(),
            )]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Int4range(
                postgres_types::ValuesRange::new(Bound::Included(1), Bound::Excluded(4)),
            )]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Int8range(
                postgres_types::ValuesRange::new(Bound::Included(1), Bound::Unbounded),
            )]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Numrange(
                postgres_types::ValuesRange::new(
                    Bound::Included(BigDecimal::from(11)),
                    Bound::Excluded(BigDecimal::from(221)),
                ),
            )]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Tsrange(tsbounds)]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Tstzrange(tstzbounds)]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Daterange(
                postgres_types::ValuesRange::new(
                    Bound::Included(chrono::NaiveDate::from_ymd_opt(2023, 2, 3).unwrap()),
                    Bound::Unbounded,
                ),
            )]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Money(1234)]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Jsonpath(
                "$.user.addresses[0].city".to_string(),
            )]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Text(
                "'a' 'and' 'ate' 'cat' 'fat' 'mat' 'on' 'rat' 'sat'".to_string(),
            )]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Text(
                "'fat' & 'rat' & !'cat'".to_string(),
            )]),
            postgres_types::DbValue::Array(vec![
                postgres_types::DbValue::Composite(postgres_types::Composite::new(
                    "a_inventory_item".to_string(),
                    vec![
                        postgres_types::DbValue::Uuid(Uuid::new_v4()),
                        postgres_types::DbValue::Text("text".to_string()),
                        postgres_types::DbValue::Int4(3),
                        postgres_types::DbValue::Numeric(BigDecimal::from(111)),
                    ],
                )),
                postgres_types::DbValue::Composite(postgres_types::Composite::new(
                    "a_inventory_item".to_string(),
                    vec![
                        postgres_types::DbValue::Uuid(Uuid::new_v4()),
                        postgres_types::DbValue::Text("text".to_string()),
                        postgres_types::DbValue::Int4(4),
                        postgres_types::DbValue::Numeric(BigDecimal::from(111)),
                    ],
                )),
            ]),
            postgres_types::DbValue::Array(vec![
                postgres_types::DbValue::Domain(Box::new(postgres_types::Domain::new(
                    "posint8".to_string(),
                    postgres_types::DbValue::Int8(1),
                ))),
                postgres_types::DbValue::Domain(Box::new(postgres_types::Domain::new(
                    "posint8".to_string(),
                    postgres_types::DbValue::Int8(2),
                ))),
            ]),
            postgres_types::DbValue::Array(vec![
                postgres_types::DbValue::Composite(postgres_types::Composite::new(
                    "ccc".to_string(),
                    vec![
                        postgres_types::DbValue::Varchar("v1".to_string()),
                        postgres_types::DbValue::Int2(1),
                        postgres_types::DbValue::Array(vec![postgres_types::DbValue::Domain(
                            Box::new(postgres_types::Domain::new(
                                "ddd".to_string(),
                                postgres_types::DbValue::Varchar("v11".to_string()),
                            )),
                        )]),
                    ],
                )),
                postgres_types::DbValue::Composite(postgres_types::Composite::new(
                    "ccc".to_string(),
                    vec![
                        postgres_types::DbValue::Varchar("v2".to_string()),
                        postgres_types::DbValue::Int2(2),
                        postgres_types::DbValue::Array(vec![
                            postgres_types::DbValue::Domain(Box::new(postgres_types::Domain::new(
                                "ddd".to_string(),
                                postgres_types::DbValue::Varchar("v21".to_string()),
                            ))),
                            postgres_types::DbValue::Domain(Box::new(postgres_types::Domain::new(
                                "ddd".to_string(),
                                postgres_types::DbValue::Varchar("v22".to_string()),
                            ))),
                        ]),
                    ],
                )),
                postgres_types::DbValue::Composite(postgres_types::Composite::new(
                    "ccc".to_string(),
                    vec![
                        postgres_types::DbValue::Varchar("v3".to_string()),
                        postgres_types::DbValue::Int2(3),
                        postgres_types::DbValue::Array(vec![
                            postgres_types::DbValue::Domain(Box::new(postgres_types::Domain::new(
                                "ddd".to_string(),
                                postgres_types::DbValue::Varchar("v31".to_string()),
                            ))),
                            postgres_types::DbValue::Domain(Box::new(postgres_types::Domain::new(
                                "ddd".to_string(),
                                postgres_types::DbValue::Varchar("v32".to_string()),
                            ))),
                            postgres_types::DbValue::Domain(Box::new(postgres_types::Domain::new(
                                "ddd".to_string(),
                                postgres_types::DbValue::Varchar("v33".to_string()),
                            ))),
                        ]),
                    ],
                )),
            ]),
            postgres_types::DbValue::Array(vec![
                postgres_types::DbValue::Range(Box::new(postgres_types::Range::new(
                    "float4range".to_string(),
                    postgres_types::ValuesRange::new(Bound::Unbounded, Bound::Unbounded),
                ))),
                postgres_types::DbValue::Range(Box::new(postgres_types::Range::new(
                    "float4range".to_string(),
                    postgres_types::ValuesRange::new(
                        Bound::Unbounded,
                        Bound::Excluded(postgres_types::DbValue::Float4(6.55)),
                    ),
                ))),
                postgres_types::DbValue::Range(Box::new(postgres_types::Range::new(
                    "float4range".to_string(),
                    postgres_types::ValuesRange::new(
                        Bound::Included(postgres_types::DbValue::Float4(2.23)),
                        Bound::Excluded(postgres_types::DbValue::Float4(4.55)),
                    ),
                ))),
                postgres_types::DbValue::Range(Box::new(postgres_types::Range::new(
                    "float4range".to_string(),
                    postgres_types::ValuesRange::new(
                        Bound::Included(postgres_types::DbValue::Float4(1.23)),
                        Bound::Unbounded,
                    ),
                ))),
            ]),
            postgres_types::DbValue::Domain(Box::new(postgres_types::Domain::new(
                "ddd".to_string(),
                postgres_types::DbValue::Varchar("tag2".to_string()),
            ))),
            postgres_types::DbValue::Range(Box::new(postgres_types::Range::new(
                "a_custom_type_range".to_string(),
                postgres_types::ValuesRange::new(
                    Bound::Included(postgres_types::DbValue::Composite(
                        postgres_types::Composite::new(
                            "a_custom_type".to_string(),
                            vec![postgres_types::DbValue::Int4(22)],
                        ),
                    )),
                    Bound::Unbounded,
                ),
            ))),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Vector(vec![
                1.0, 2.0, 3.0, 4.0, 5.0,
            ])]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Halfvec(
                half::vec::HalfFloatVecExt::from_f32_slice(&[1.0, 2.0, 3.0, 4.0, 5.0]),
            )]),
            postgres_types::DbValue::Array(vec![postgres_types::DbValue::Sparsevec(
                postgres_types::SparseVec::try_new(5, vec![1, 2, 4], vec![1.0, 2.0, 4.0]).unwrap(),
            )]),
        ]
    }

    mod roundtrip_tests {
        use super::super::*;
        use golem_common::model::oplog::payload::types::SerializableDbValue;
        use test_r::test;

        #[test]
        fn test_dbvalue_roundtrip_simple_types() {
            let test_values = vec![
                DbValue::Int4(42),
                DbValue::Text("hello".to_string()),
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
        fn test_dbvalue_roundtrip_arrays() {
            let test_values = vec![
                DbValue::Array(vec![DbValue::Int4(1), DbValue::Int4(2), DbValue::Int4(3)]),
                DbValue::Array(vec![
                    DbValue::Text("a".to_string()),
                    DbValue::Text("b".to_string()),
                ]),
                DbValue::Array(vec![]),
            ];

            for original in test_values {
                let serialized: SerializableDbValue = original.clone().into();
                let deserialized: DbValue = serialized.try_into().expect("deserialization failed");
                assert_eq!(original, deserialized, "roundtrip failed for array");
            }
        }

        #[test]
        fn test_dbvalue_roundtrip_composite() {
            let original = DbValue::Composite(Composite::new(
                "test_composite".to_string(),
                vec![DbValue::Int4(42), DbValue::Text("hello".to_string())],
            ));

            let serialized: SerializableDbValue = original.clone().into();
            let deserialized: DbValue = serialized.try_into().expect("deserialization failed");
            assert_eq!(original, deserialized, "roundtrip failed for composite");
        }

        #[test]
        fn test_dbvalue_roundtrip_domain() {
            let original = DbValue::Domain(Box::new(Domain::new(
                "my_domain".to_string(),
                DbValue::Int8(100),
            )));

            let serialized: SerializableDbValue = original.clone().into();
            let deserialized: DbValue = serialized.try_into().expect("deserialization failed");
            assert_eq!(original, deserialized, "roundtrip failed for domain");
        }

        #[test]
        fn test_dbvalue_roundtrip_range() {
            let original = DbValue::Range(Box::new(Range::new(
                "int4range".to_string(),
                ValuesRange::new(
                    std::collections::Bound::Included(DbValue::Int4(1)),
                    std::collections::Bound::Excluded(DbValue::Int4(10)),
                ),
            )));

            let serialized: SerializableDbValue = original.clone().into();
            let deserialized: DbValue = serialized.try_into().expect("deserialization failed");
            assert_eq!(original, deserialized, "roundtrip failed for range");
        }

        #[test]
        fn test_dbvalue_roundtrip_all_test_values() {
            let test_values = super::get_test_db_values();

            for (idx, original) in test_values.into_iter().enumerate() {
                let serialized: SerializableDbValue = original.clone().into();
                let deserialized: DbValue = serialized
                    .try_into()
                    .unwrap_or_else(|_| panic!("deserialization failed for test value {}", idx));
                assert_eq!(
                    original, deserialized,
                    "roundtrip failed for test value {} at index {}",
                    idx, idx
                );
            }
        }
    }
}
