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

use crate::services::rdbms::{
    get_bound_analysed_type, get_bound_value, AnalysedTypeMerger, RdbmsIntoValueAndType,
};
use bigdecimal::BigDecimal;
use bincode::{Decode, Encode};
use bit_vec::BitVec;
use golem_wasm_ast::analysis::{analysed_type, AnalysedType};
use golem_wasm_rpc::{IntoValue, Value, ValueAndType};
use golem_wasm_rpc_derive::IntoValue;
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

#[derive(Clone, Debug, Eq, PartialEq, Encode, Decode, IntoValue)]
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

#[derive(Clone, Debug, Eq, PartialEq, Encode, Decode)]
pub struct CompositeType {
    pub name: String,
    pub attributes: Vec<(String, DbColumnType)>,
}

impl CompositeType {
    pub fn new(name: String, attributes: Vec<(String, DbColumnType)>) -> Self {
        CompositeType { name, attributes }
    }

    fn get_analysed_type(attribute_type: AnalysedType) -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("name", analysed_type::str()),
            analysed_type::field(
                "attributes",
                analysed_type::list(analysed_type::tuple(vec![
                    analysed_type::str(),
                    attribute_type,
                ])),
            ),
        ])
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

impl RdbmsIntoValueAndType for CompositeType {
    fn into_value_and_type(self) -> ValueAndType {
        let mut vs = Vec::with_capacity(self.attributes.len());
        let mut t: Option<AnalysedType> = None;
        for (n, v) in self.attributes {
            let v = v.into_value_and_type();
            t = match t {
                None => Some(v.typ),
                Some(t) => Some(DbColumnType::merge_types(t, v.typ)),
            };
            vs.push(Value::Tuple(vec![n.into_value(), v.value]));
        }
        let typ = Self::get_analysed_type(t.unwrap_or(DbColumnType::get_base_type()));
        let value = Value::Record(vec![self.name.into_value(), Value::List(vs)]);
        ValueAndType::new(value, typ)
    }

    fn get_base_type() -> AnalysedType {
        Self::get_analysed_type(DbColumnType::get_base_type())
    }
}

impl AnalysedTypeMerger for CompositeType {
    fn merge_types(first: AnalysedType, second: AnalysedType) -> AnalysedType {
        fn get_attribute_type(attributes_type: AnalysedType) -> Option<AnalysedType> {
            if let AnalysedType::List(attrs) = attributes_type {
                if let AnalysedType::Tuple(attr) = *attrs.inner {
                    attr.items.get(1).cloned()
                } else {
                    None
                }
            } else {
                None
            }
        }

        if let (AnalysedType::Record(f), AnalysedType::Record(s)) = (first.clone(), second) {
            if f.fields.len() == s.fields.len() {
                let mut fields = Vec::with_capacity(f.fields.len());
                let mut ok = true;

                for (fc, sc) in f.fields.into_iter().zip(s.fields.into_iter()) {
                    if fc.name != sc.name {
                        ok = false;
                        break;
                    }
                    if fc.name == "attributes" {
                        let f = get_attribute_type(fc.typ.clone());
                        let s = get_attribute_type(sc.typ);
                        let t = DbColumnType::merge_types_opt(f, s);

                        if let Some(t) = t {
                            fields.push(analysed_type::field(
                                fc.name.as_str(),
                                analysed_type::list(analysed_type::tuple(vec![
                                    analysed_type::str(),
                                    t,
                                ])),
                            ));
                        } else {
                            fields.push(fc);
                        }
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

    fn get_analysed_type(base_type: AnalysedType) -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("name", analysed_type::str()),
            analysed_type::field("base-type", base_type),
        ])
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

impl RdbmsIntoValueAndType for DomainType {
    fn into_value_and_type(self) -> ValueAndType {
        let v = RdbmsIntoValueAndType::into_value_and_type(*self.base_type);
        let typ = Self::get_analysed_type(v.typ);
        let value = Value::Record(vec![self.name.into_value(), v.value]);
        ValueAndType::new(value, typ)
    }

    fn get_base_type() -> AnalysedType {
        Self::get_analysed_type(DbColumnType::get_base_type())
    }
}

impl AnalysedTypeMerger for DomainType {
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
                    if fc.name == "base-type" {
                        let t = DbColumnType::merge_types(fc.typ, sc.typ);
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

    fn get_analysed_type(base_type: AnalysedType) -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("name", analysed_type::str()),
            analysed_type::field("base-type", base_type),
        ])
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

impl RdbmsIntoValueAndType for RangeType {
    fn into_value_and_type(self) -> ValueAndType {
        let v = RdbmsIntoValueAndType::into_value_and_type(*self.base_type);
        let typ = Self::get_analysed_type(v.typ);
        let value = Value::Record(vec![self.name.into_value(), v.value]);
        ValueAndType::new(value, typ)
    }

    fn get_base_type() -> AnalysedType {
        Self::get_analysed_type(DbColumnType::get_base_type())
    }
}

impl AnalysedTypeMerger for RangeType {
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
                    if fc.name == "base-type" {
                        let t = DbColumnType::merge_types(fc.typ, sc.typ);
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

    fn get_analysed_type(bound_type: AnalysedType) -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("start", bound_type.clone()),
            analysed_type::field("end", bound_type.clone()),
        ])
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
        Self::get_analysed_type(Bound::<T>::get_type())
    }
}

#[derive(Clone, Debug, PartialEq, Encode, Decode, IntoValue)]
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Encode, Decode, IntoValue)]
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

#[derive(Clone, Debug, PartialEq, Encode, Decode, IntoValue)]
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

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct Composite {
    pub name: String,
    pub values: Vec<DbValue>,
}

impl Composite {
    pub fn new(name: String, values: Vec<DbValue>) -> Self {
        Composite { name, values }
    }

    fn get_analysed_type(values_type: AnalysedType) -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("name", analysed_type::str()),
            analysed_type::field("values", values_type),
        ])
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

impl RdbmsIntoValueAndType for Composite {
    fn into_value_and_type(self) -> ValueAndType {
        let values = RdbmsIntoValueAndType::into_value_and_type(self.values);
        let typ = Self::get_analysed_type(values.typ);
        let value = Value::Record(vec![self.name.into_value(), values.value]);
        ValueAndType::new(value, typ)
    }
    fn get_base_type() -> AnalysedType {
        Self::get_analysed_type(<Vec<DbValue>>::get_base_type())
    }
}

impl AnalysedTypeMerger for Composite {
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
                        let t = <Vec<DbValue>>::merge_types(fc.typ, sc.typ);
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

    fn get_analysed_type(value_type: AnalysedType) -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("name", analysed_type::str()),
            analysed_type::field("value", value_type),
        ])
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

impl RdbmsIntoValueAndType for Domain {
    fn into_value_and_type(self) -> ValueAndType {
        let v = RdbmsIntoValueAndType::into_value_and_type(*self.value);
        let typ = Self::get_analysed_type(v.typ);
        let value = Value::Record(vec![self.name.into_value(), v.value]);
        ValueAndType::new(value, typ)
    }

    fn get_base_type() -> AnalysedType {
        Self::get_analysed_type(DbColumnType::get_base_type())
    }
}

impl AnalysedTypeMerger for Domain {
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
                    if fc.name == "value" {
                        let t = DbValue::merge_types(fc.typ, sc.typ);
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

    fn get_value_analysed_type(base_type: AnalysedType) -> AnalysedType {
        let base_type = get_bound_analysed_type(base_type);
        ValuesRange::<DbValue>::get_analysed_type(base_type)
    }

    fn get_analysed_type(base_type: AnalysedType) -> AnalysedType {
        let value_type = Self::get_value_analysed_type(base_type);

        analysed_type::record(vec![
            analysed_type::field("name", analysed_type::str()),
            analysed_type::field("value", value_type),
        ])
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

impl RdbmsIntoValueAndType for Range {
    fn into_value_and_type(self) -> ValueAndType {
        let (value_start, typ_start) = get_bound_value(self.value.start);
        let (value_end, typ_end) = get_bound_value(self.value.end);

        let base_type =
            DbValue::merge_types_opt(typ_start, typ_end).unwrap_or(DbValue::get_base_type());

        let typ = Self::get_analysed_type(base_type);

        let value = Value::Record(vec![
            self.name.into_value(),
            Value::Record(vec![value_start, value_end]),
        ]);
        ValueAndType::new(value, typ)
    }

    fn get_base_type() -> AnalysedType {
        Self::get_analysed_type(DbValue::get_base_type())
    }
}

impl AnalysedTypeMerger for Range {
    fn merge_types(first: AnalysedType, second: AnalysedType) -> AnalysedType {
        fn get_bound_type(base_type: AnalysedType) -> Option<AnalysedType> {
            if let AnalysedType::Variant(cases) = base_type {
                if cases.cases.len() == 3 {
                    cases.cases[0].typ.clone().or(cases.cases[1].typ.clone())
                } else {
                    None
                }
            } else {
                None
            }
        }

        fn get_value_type(attributes_type: AnalysedType) -> Option<AnalysedType> {
            if let AnalysedType::Record(fields) = attributes_type {
                if fields.fields.len() == 2 {
                    get_bound_type(fields.fields[0].typ.clone())
                        .or(get_bound_type(fields.fields[1].typ.clone()))
                } else {
                    None
                }
            } else {
                None
            }
        }

        if let (AnalysedType::Record(f), AnalysedType::Record(s)) = (first.clone(), second) {
            if f.fields.len() == s.fields.len() {
                let mut fields = Vec::with_capacity(f.fields.len());
                let mut ok = true;

                for (fc, sc) in f.fields.into_iter().zip(s.fields.into_iter()) {
                    if fc.name != sc.name {
                        ok = false;
                        break;
                    }
                    if fc.name == "value" {
                        let f = get_value_type(fc.typ.clone());
                        let s = get_value_type(sc.typ);
                        let t = DbValue::merge_types_opt(f, s);

                        if let Some(t) = t {
                            fields.push(analysed_type::field(
                                fc.name.as_str(),
                                Range::get_value_analysed_type(t),
                            ));
                        } else {
                            fields.push(fc);
                        }
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
    Array(Box<DbColumnType>),
    Range(RangeType),
    Null,
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

    fn get_analysed_type(
        composite_type: Option<AnalysedType>,
        domain_type: Option<AnalysedType>,
        array_type: Option<AnalysedType>,
        range_type: Option<AnalysedType>,
    ) -> AnalysedType {
        let composite_type = analysed_type::opt_case("composite", composite_type);
        let domain_type = analysed_type::opt_case("domain", domain_type);
        let array_type = analysed_type::opt_case("array", array_type);
        let range_type = analysed_type::opt_case("range", range_type);

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
            composite_type,
            domain_type,
            array_type,
            range_type,
            analysed_type::unit_case("null"),
        ])
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

impl RdbmsIntoValueAndType for DbColumnType {
    fn into_value_and_type(self) -> ValueAndType {
        fn get_variant(case_idx: u32, case_value: Option<Value>) -> ValueAndType {
            let case_value = case_value.map(Box::new);
            let v = Value::Variant {
                case_idx,
                case_value,
            };
            ValueAndType::new(v, DbColumnType::get_analysed_type(None, None, None, None))
        }

        match self {
            DbColumnType::Character => get_variant(0, None),
            DbColumnType::Int2 => get_variant(1, None),
            DbColumnType::Int4 => get_variant(2, None),
            DbColumnType::Int8 => get_variant(3, None),
            DbColumnType::Float4 => get_variant(4, None),
            DbColumnType::Float8 => get_variant(5, None),
            DbColumnType::Numeric => get_variant(6, None),
            DbColumnType::Boolean => get_variant(7, None),
            DbColumnType::Text => get_variant(8, None),
            DbColumnType::Varchar => get_variant(9, None),
            DbColumnType::Bpchar => get_variant(10, None),
            DbColumnType::Timestamp => get_variant(11, None),
            DbColumnType::Timestamptz => get_variant(12, None),
            DbColumnType::Date => get_variant(13, None),
            DbColumnType::Time => get_variant(14, None),
            DbColumnType::Timetz => get_variant(15, None),
            DbColumnType::Interval => get_variant(16, None),
            DbColumnType::Bytea => get_variant(17, None),
            DbColumnType::Uuid => get_variant(18, None),
            DbColumnType::Xml => get_variant(19, None),
            DbColumnType::Json => get_variant(29, None),
            DbColumnType::Jsonb => get_variant(21, None),
            DbColumnType::Jsonpath => get_variant(22, None),
            DbColumnType::Inet => get_variant(23, None),
            DbColumnType::Cidr => get_variant(24, None),
            DbColumnType::Macaddr => get_variant(25, None),
            DbColumnType::Bit => get_variant(26, None),
            DbColumnType::Varbit => get_variant(27, None),
            DbColumnType::Int4range => get_variant(28, None),
            DbColumnType::Int8range => get_variant(29, None),
            DbColumnType::Numrange => get_variant(30, None),
            DbColumnType::Tsrange => get_variant(31, None),
            DbColumnType::Tstzrange => get_variant(32, None),
            DbColumnType::Daterange => get_variant(33, None),
            DbColumnType::Money => get_variant(34, None),
            DbColumnType::Oid => get_variant(35, None),
            DbColumnType::Enumeration(v) => get_variant(36, Some(v.into_value())),
            DbColumnType::Composite(v) => {
                let v = v.into_value_and_type();
                let value = Value::Variant {
                    case_idx: 37,
                    case_value: Some(Box::new(v.value)),
                };
                ValueAndType::new(
                    value,
                    DbColumnType::get_analysed_type(Some(v.typ), None, None, None),
                )
            }
            DbColumnType::Domain(v) => {
                let v = v.into_value_and_type();
                let value = Value::Variant {
                    case_idx: 38,
                    case_value: Some(Box::new(v.value)),
                };
                ValueAndType::new(
                    value,
                    DbColumnType::get_analysed_type(None, Some(v.typ), None, None),
                )
            }
            DbColumnType::Array(v) => {
                let v = v.into_value_and_type();
                let value = Value::Variant {
                    case_idx: 39,
                    case_value: Some(Box::new(v.value)),
                };
                ValueAndType::new(
                    value,
                    DbColumnType::get_analysed_type(None, None, Some(v.typ), None),
                )
            }
            DbColumnType::Range(v) => {
                let v = v.into_value_and_type();
                let value = Value::Variant {
                    case_idx: 40,
                    case_value: Some(Box::new(v.value)),
                };
                ValueAndType::new(
                    value,
                    DbColumnType::get_analysed_type(None, None, None, Some(v.typ)),
                )
            }
            DbColumnType::Null => get_variant(41, None),
        }
    }

    fn get_base_type() -> AnalysedType {
        DbColumnType::get_analysed_type(None, None, None, None)
    }
}

impl AnalysedTypeMerger for DbColumnType {
    fn merge_types(first: AnalysedType, second: AnalysedType) -> AnalysedType {
        if let (AnalysedType::Variant(f), AnalysedType::Variant(s)) = (first.clone(), second) {
            if f.cases.len() == s.cases.len() {
                let mut cases = Vec::with_capacity(f.cases.len());
                let mut ok = true;

                for (fc, sc) in f.cases.into_iter().zip(s.cases.into_iter()) {
                    if fc.name != sc.name {
                        ok = false;
                        break;
                    }

                    if fc.name == "composite" {
                        let t = CompositeType::merge_types_opt(fc.typ, sc.typ);
                        cases.push(analysed_type::opt_case(fc.name.as_str(), t));
                    } else if fc.name == "range" {
                        let t = RangeType::merge_types_opt(fc.typ, sc.typ);
                        cases.push(analysed_type::opt_case(fc.name.as_str(), t));
                    } else if fc.name == "domain" {
                        let t = DomainType::merge_types_opt(fc.typ, sc.typ);
                        cases.push(analysed_type::opt_case(fc.name.as_str(), t));
                    } else if fc.name == "array" {
                        let t = DbColumnType::merge_types_opt(fc.typ, sc.typ);
                        cases.push(analysed_type::opt_case(fc.name.as_str(), t));
                    } else {
                        cases.push(fc);
                    }
                }
                if ok {
                    analysed_type::variant(cases)
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
    Array(Vec<DbValue>),
    Range(Range),
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

    fn get_analysed_type(
        composite_type: Option<AnalysedType>,
        domain_type: Option<AnalysedType>,
        array_type: Option<AnalysedType>,
        range_type: Option<AnalysedType>,
    ) -> AnalysedType {
        let composite_type = analysed_type::opt_case("composite", composite_type);
        let domain_type = analysed_type::opt_case("domain", domain_type);
        let array_type = analysed_type::opt_case("array", array_type);
        let range_type = analysed_type::opt_case("range", range_type);

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
            analysed_type::case("timestamp", chrono::NaiveDateTime::get_type()),
            analysed_type::case("timestamptz", chrono::DateTime::<chrono::Utc>::get_type()),
            analysed_type::case("date", chrono::NaiveDate::get_type()),
            analysed_type::case("time", chrono::NaiveTime::get_type()),
            analysed_type::case("timetz", TimeTz::get_type()),
            analysed_type::case("interval", Interval::get_type()),
            analysed_type::case("bytea", analysed_type::list(analysed_type::u8())),
            analysed_type::case("json", analysed_type::str()),
            analysed_type::case("jsonb", analysed_type::str()),
            analysed_type::case("jsonpath", analysed_type::str()),
            analysed_type::case("xml", analysed_type::str()),
            analysed_type::case(
                "uuid",
                analysed_type::record(vec![
                    analysed_type::field("high-bits", analysed_type::u64()),
                    analysed_type::field("low-bits", analysed_type::u64()),
                ]),
            ),
            analysed_type::case("inet", IpAddr::get_base_type()),
            analysed_type::case("cidr", IpAddr::get_base_type()),
            analysed_type::case("macaddr", MacAddress::get_base_type()),
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
            composite_type,
            domain_type,
            array_type,
            range_type,
            analysed_type::unit_case("null"),
        ])
    }
}

impl RdbmsIntoValueAndType for DbValue {
    fn into_value_and_type(self) -> ValueAndType {
        fn get_variant(case_idx: u32, case_value: Option<Value>) -> ValueAndType {
            let case_value = case_value.map(Box::new);
            let v = Value::Variant {
                case_idx,
                case_value,
            };
            ValueAndType::new(v, DbValue::get_analysed_type(None, None, None, None))
        }

        match self {
            DbValue::Character(v) => get_variant(0, Some(v.into_value())),
            DbValue::Int2(v) => get_variant(1, Some(v.into_value())),
            DbValue::Int4(v) => get_variant(2, Some(v.into_value())),
            DbValue::Int8(v) => get_variant(3, Some(v.into_value())),
            DbValue::Float4(v) => get_variant(4, Some(v.into_value())),
            DbValue::Float8(v) => get_variant(5, Some(v.into_value())),
            DbValue::Numeric(v) => get_variant(6, Some(v.to_string().into_value())),
            DbValue::Boolean(v) => get_variant(7, Some(v.into_value())),
            DbValue::Text(v) => get_variant(8, Some(v.into_value())),
            DbValue::Varchar(v) => get_variant(9, Some(v.into_value())),
            DbValue::Bpchar(v) => get_variant(10, Some(v.into_value())),
            DbValue::Timestamp(v) => get_variant(11, Some(v.into_value())),
            DbValue::Timestamptz(v) => get_variant(12, Some(v.into_value())),
            DbValue::Date(v) => get_variant(13, Some(v.into_value())),
            DbValue::Time(v) => get_variant(14, Some(v.into_value())),
            DbValue::Timetz(v) => get_variant(15, Some(v.into_value())),
            DbValue::Interval(v) => get_variant(16, Some(v.into_value())),
            DbValue::Bytea(v) => get_variant(17, Some(v.into_value())),
            DbValue::Json(v) => get_variant(18, Some(v.into_value())),
            DbValue::Jsonb(v) => get_variant(19, Some(v.into_value())),
            DbValue::Jsonpath(v) => get_variant(20, Some(v.into_value())),
            DbValue::Xml(v) => get_variant(21, Some(v.into_value())),
            DbValue::Uuid(v) => {
                let (h, l) = v.as_u64_pair();
                let v = Value::Record(vec![Value::U64(h), Value::U64(l)]);
                get_variant(22, Some(v))
            }
            DbValue::Inet(v) => get_variant(23, Some(v.into_value_and_type().value)),
            DbValue::Cidr(v) => get_variant(24, Some(v.into_value_and_type().value)),
            DbValue::Macaddr(v) => get_variant(25, Some(v.into_value_and_type().value)),
            DbValue::Bit(v) => get_variant(26, Some(v.iter().collect::<Vec<bool>>().into_value())),
            DbValue::Varbit(v) => {
                get_variant(27, Some(v.iter().collect::<Vec<bool>>().into_value()))
            }
            DbValue::Int4range(v) => get_variant(28, Some(v.into_value())),
            DbValue::Int8range(v) => get_variant(29, Some(v.into_value())),
            DbValue::Numrange(v) => get_variant(30, Some(v.map(|v| v.to_string()).into_value())),
            DbValue::Tsrange(v) => get_variant(31, Some(v.map(|v| v.to_string()).into_value())),
            DbValue::Tstzrange(v) => get_variant(32, Some(v.map(|v| v.to_string()).into_value())),
            DbValue::Daterange(v) => get_variant(33, Some(v.map(|v| v.to_string()).into_value())),
            DbValue::Money(v) => get_variant(34, Some(v.into_value())),
            DbValue::Oid(v) => get_variant(35, Some(v.into_value())),
            DbValue::Enumeration(v) => get_variant(36, Some(v.into_value())),
            DbValue::Composite(v) => {
                let v = v.into_value_and_type();
                let value = Value::Variant {
                    case_idx: 37,
                    case_value: Some(Box::new(v.value)),
                };
                ValueAndType::new(
                    value,
                    DbValue::get_analysed_type(Some(v.typ), None, None, None),
                )
            }
            DbValue::Domain(v) => {
                let v = v.into_value_and_type();
                let value = Value::Variant {
                    case_idx: 38,
                    case_value: Some(Box::new(v.value)),
                };
                ValueAndType::new(
                    value,
                    DbValue::get_analysed_type(None, Some(v.typ), None, None),
                )
            }
            DbValue::Array(v) => {
                let v = v.into_value_and_type();
                let value = Value::Variant {
                    case_idx: 39,
                    case_value: Some(Box::new(v.value)),
                };
                ValueAndType::new(
                    value,
                    DbValue::get_analysed_type(None, None, Some(v.typ), None),
                )
            }
            DbValue::Range(v) => {
                let v = v.into_value_and_type();
                let value = Value::Variant {
                    case_idx: 40,
                    case_value: Some(Box::new(v.value)),
                };
                ValueAndType::new(
                    value,
                    DbValue::get_analysed_type(None, None, None, Some(v.typ)),
                )
            }
            DbValue::Null => get_variant(41, None),
        }
    }

    fn get_base_type() -> AnalysedType {
        DbValue::get_analysed_type(None, None, None, None)
    }
}

impl AnalysedTypeMerger for DbValue {
    fn merge_types(first: AnalysedType, second: AnalysedType) -> AnalysedType {
        if let (AnalysedType::Variant(f), AnalysedType::Variant(s)) = (first.clone(), second) {
            if f.cases.len() == s.cases.len() {
                let mut cases = Vec::with_capacity(f.cases.len());
                let mut ok = true;

                for (fc, sc) in f.cases.into_iter().zip(s.cases.into_iter()) {
                    if fc.name != sc.name {
                        ok = false;
                        break;
                    }

                    if fc.name == "composite" {
                        let t = Composite::merge_types_opt(fc.typ, sc.typ);
                        cases.push(analysed_type::opt_case(fc.name.as_str(), t));
                    } else if fc.name == "range" {
                        let t = Range::merge_types_opt(fc.typ, sc.typ);
                        cases.push(analysed_type::opt_case(fc.name.as_str(), t));
                    } else if fc.name == "domain" {
                        let t = Domain::merge_types_opt(fc.typ, sc.typ);
                        cases.push(analysed_type::opt_case(fc.name.as_str(), t));
                    } else if fc.name == "array" {
                        let t = <Vec<DbValue>>::merge_types_opt(fc.typ, sc.typ);
                        cases.push(analysed_type::opt_case(fc.name.as_str(), t));
                    } else {
                        cases.push(fc);
                    }
                }
                if ok {
                    analysed_type::variant(cases)
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

#[derive(Clone, Debug, Eq, PartialEq, Encode, Decode)]
pub struct DbColumn {
    pub ordinal: u64,
    pub name: String,
    pub db_type: DbColumnType,
    pub db_type_name: String,
}

impl DbColumn {
    fn get_analysed_type(column_type: AnalysedType) -> AnalysedType {
        analysed_type::record(vec![
            analysed_type::field("ordinal", analysed_type::u64()),
            analysed_type::field("name", analysed_type::str()),
            analysed_type::field("db-type", column_type),
            analysed_type::field("db-type-name", analysed_type::str()),
        ])
    }
}

impl RdbmsIntoValueAndType for DbColumn {
    fn into_value_and_type(self) -> ValueAndType {
        let db_type = RdbmsIntoValueAndType::into_value_and_type(self.db_type);
        let t = DbColumn::get_analysed_type(db_type.typ);
        let v = Value::Record(vec![
            self.ordinal.into_value(),
            self.name.into_value(),
            db_type.value,
            self.db_type_name.into_value(),
        ]);
        ValueAndType::new(v, t)
    }

    fn get_base_type() -> AnalysedType {
        DbColumn::get_analysed_type(DbColumnType::get_base_type())
    }
}

impl AnalysedTypeMerger for DbColumn {
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
                    if fc.name == "db-type" {
                        let t = DbColumnType::merge_types(fc.typ, sc.typ);
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

#[cfg(test)]
pub mod tests {
    use crate::services::rdbms::postgres::{types as postgres_types, PostgresType};
    use crate::services::rdbms::{AnalysedTypeMerger, DbResult, DbRow, RdbmsIntoValueAndType};
    use assert2::check;
    use bigdecimal::BigDecimal;
    use bincode::{Decode, Encode};
    use bit_vec::BitVec;
    use golem_common::serialization::{serialize, try_deserialize};
    use mac_address::MacAddress;
    use serde_json::json;
    use std::collections::Bound;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
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

    fn check_type_merge<T: RdbmsIntoValueAndType + AnalysedTypeMerger>(
        value1: T,
        value2: T,
        expected: T,
    ) {
        let vt1 = value1.into_value_and_type();
        let vt2 = value2.into_value_and_type();

        let vt_merged = T::merge_types(vt1.typ, vt2.typ);
        let vt_merged_json = serde_json::to_string(&vt_merged);

        let vt_expected = expected.into_value_and_type();
        let vt_expected_json = serde_json::to_string(&vt_expected);

        check!(vt_merged == vt_expected.typ);
        check!(vt_merged_json.is_ok());
        check!(vt_expected_json.is_ok());
    }

    #[test]
    fn test_db_value_analysed_type_merge() {
        for (value1, value2, value) in get_test_db_values_values() {
            check_type_merge(value1.clone(), value2.clone(), value.clone());
            check_type_merge(value2.clone(), value1.clone(), value.clone());
            check_type_merge(value1.clone(), value1.clone(), value1.clone());
        }
    }

    #[test]
    fn test_db_values_conversions() {
        let values = get_test_db_values();

        for value in values {
            check_bincode(value.clone());
            check_type_and_value(value);
        }
    }

    #[test]
    fn test_db_column_types_conversions() {
        let values = get_test_db_column_types();

        for value in values {
            check_bincode(value.clone());
            check_type_and_value(value);
        }
    }

    #[test]
    fn test_db_result_conversions() {
        let value = DbResult::<PostgresType>::new(
            get_test_db_columns(),
            vec![DbRow {
                values: get_test_db_values(),
            }],
        );

        check_bincode(value.clone());
        check_type_and_value(value);
    }

    #[test]
    fn test_db_column_type_analysed_type_merge() {
        for (value1, value2, value) in get_test_db_column_types_values() {
            check_type_merge(value1.clone(), value2.clone(), value.clone());
            check_type_merge(value2.clone(), value1.clone(), value.clone());
            check_type_merge(value1.clone(), value1.clone(), value1.clone());
        }
    }

    fn get_test_db_values_values() -> Vec<(
        postgres_types::DbValue,
        postgres_types::DbValue,
        postgres_types::DbValue,
    )> {
        let mut values: Vec<(
            postgres_types::DbValue,
            postgres_types::DbValue,
            postgres_types::DbValue,
        )> = vec![];

        let value1 = postgres_types::DbValue::Composite(postgres_types::Composite::new(
            "ccc1".to_string(),
            vec![
                postgres_types::DbValue::Int2(3),
                postgres_types::DbValue::Domain(postgres_types::Domain::new(
                    "ddd".to_string(),
                    postgres_types::DbValue::Varchar("v31".to_string()),
                )),
                postgres_types::DbValue::Range(postgres_types::Range::new(
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
                )),
            ],
        ));

        let value2 = postgres_types::DbValue::Composite(postgres_types::Composite::new(
            "ccc2".to_string(),
            vec![
                postgres_types::DbValue::Varchar("v3".to_string()),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Domain(
                    postgres_types::Domain::new(
                        "ddd".to_string(),
                        postgres_types::DbValue::Varchar("v31".to_string()),
                    ),
                )]),
            ],
        ));

        let value = postgres_types::DbValue::Composite(postgres_types::Composite::new(
            "ccc1".to_string(),
            vec![
                postgres_types::DbValue::Varchar("v3".to_string()),
                postgres_types::DbValue::Int2(3),
                postgres_types::DbValue::Domain(postgres_types::Domain::new(
                    "ddd".to_string(),
                    postgres_types::DbValue::Varchar("v31".to_string()),
                )),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Domain(
                    postgres_types::Domain::new(
                        "ddd".to_string(),
                        postgres_types::DbValue::Varchar("v31".to_string()),
                    ),
                )]),
                postgres_types::DbValue::Range(postgres_types::Range::new(
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
                )),
            ],
        ));

        values.push((value1.clone(), value2.clone(), value.clone()));

        let value1 = postgres_types::DbValue::Array(vec![value1]);
        let value2 = postgres_types::DbValue::Array(vec![value2]);
        let value = postgres_types::DbValue::Array(vec![value]);

        values.push((value1.clone(), value2.clone(), value.clone()));

        values
    }

    fn get_test_db_column_types_values() -> Vec<(
        postgres_types::DbColumnType,
        postgres_types::DbColumnType,
        postgres_types::DbColumnType,
    )> {
        let mut values: Vec<(
            postgres_types::DbColumnType,
            postgres_types::DbColumnType,
            postgres_types::DbColumnType,
        )> = vec![];

        let value1 = postgres_types::DbColumnType::Composite(postgres_types::CompositeType::new(
            "item1".to_string(),
            vec![
                ("product_id".to_string(), postgres_types::DbColumnType::Uuid),
                ("name".to_string(), postgres_types::DbColumnType::Text),
                (
                    "tags".to_string(),
                    postgres_types::DbColumnType::Text.into_array(),
                ),
                (
                    "supplier_id".to_string(),
                    postgres_types::DbColumnType::Int4,
                ),
            ],
        ));

        let value2 = postgres_types::DbColumnType::Composite(postgres_types::CompositeType::new(
            "item2".to_string(),
            vec![
                ("product_id".to_string(), postgres_types::DbColumnType::Uuid),
                ("name".to_string(), postgres_types::DbColumnType::Text),
                (
                    "interval".to_string(),
                    postgres_types::DbColumnType::Range(postgres_types::RangeType::new(
                        "float4range".to_string(),
                        postgres_types::DbColumnType::Float4,
                    )),
                ),
            ],
        ));

        let expected = postgres_types::DbColumnType::Composite(postgres_types::CompositeType::new(
            "item3".to_string(),
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

        values.push((value1.clone(), value2.clone(), expected.clone()));

        values.push((
            value1.into_array(),
            value2.into_array(),
            expected.into_array(),
        ));

        values
    }

    pub(crate) fn get_test_db_columns() -> Vec<postgres_types::DbColumn> {
        let types = get_test_db_column_types();
        let mut columns: Vec<postgres_types::DbColumn> = Vec::with_capacity(types.len());

        for (i, ct) in types.iter().enumerate() {
            let c = postgres_types::DbColumn {
                ordinal: i as u64,
                name: format!("column-{}", i),
                db_type: ct.clone(),
                db_type_name: ct.to_string(),
            };
            columns.push(c);
        }
        columns
    }

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

        let value = postgres_types::DbColumnType::Domain(postgres_types::DomainType::new(
            "posint8".to_string(),
            postgres_types::DbColumnType::Int8,
        ));

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
                postgres_types::DbValue::Domain(postgres_types::Domain::new(
                    "posint8".to_string(),
                    postgres_types::DbValue::Int8(1),
                )),
                postgres_types::DbValue::Domain(postgres_types::Domain::new(
                    "posint8".to_string(),
                    postgres_types::DbValue::Int8(2),
                )),
            ]),
            postgres_types::DbValue::Array(vec![
                postgres_types::DbValue::Composite(postgres_types::Composite::new(
                    "ccc".to_string(),
                    vec![
                        postgres_types::DbValue::Varchar("v1".to_string()),
                        postgres_types::DbValue::Int2(1),
                        postgres_types::DbValue::Array(vec![postgres_types::DbValue::Domain(
                            postgres_types::Domain::new(
                                "ddd".to_string(),
                                postgres_types::DbValue::Varchar("v11".to_string()),
                            ),
                        )]),
                    ],
                )),
                postgres_types::DbValue::Composite(postgres_types::Composite::new(
                    "ccc".to_string(),
                    vec![
                        postgres_types::DbValue::Varchar("v2".to_string()),
                        postgres_types::DbValue::Int2(2),
                        postgres_types::DbValue::Array(vec![
                            postgres_types::DbValue::Domain(postgres_types::Domain::new(
                                "ddd".to_string(),
                                postgres_types::DbValue::Varchar("v21".to_string()),
                            )),
                            postgres_types::DbValue::Domain(postgres_types::Domain::new(
                                "ddd".to_string(),
                                postgres_types::DbValue::Varchar("v22".to_string()),
                            )),
                        ]),
                    ],
                )),
                postgres_types::DbValue::Composite(postgres_types::Composite::new(
                    "ccc".to_string(),
                    vec![
                        postgres_types::DbValue::Varchar("v3".to_string()),
                        postgres_types::DbValue::Int2(3),
                        postgres_types::DbValue::Array(vec![
                            postgres_types::DbValue::Domain(postgres_types::Domain::new(
                                "ddd".to_string(),
                                postgres_types::DbValue::Varchar("v31".to_string()),
                            )),
                            postgres_types::DbValue::Domain(postgres_types::Domain::new(
                                "ddd".to_string(),
                                postgres_types::DbValue::Varchar("v32".to_string()),
                            )),
                            postgres_types::DbValue::Domain(postgres_types::Domain::new(
                                "ddd".to_string(),
                                postgres_types::DbValue::Varchar("v33".to_string()),
                            )),
                        ]),
                    ],
                )),
            ]),
            postgres_types::DbValue::Array(vec![
                postgres_types::DbValue::Range(postgres_types::Range::new(
                    "float4range".to_string(),
                    postgres_types::ValuesRange::new(Bound::Unbounded, Bound::Unbounded),
                )),
                postgres_types::DbValue::Range(postgres_types::Range::new(
                    "float4range".to_string(),
                    postgres_types::ValuesRange::new(
                        Bound::Unbounded,
                        Bound::Excluded(postgres_types::DbValue::Float4(6.55)),
                    ),
                )),
                postgres_types::DbValue::Range(postgres_types::Range::new(
                    "float4range".to_string(),
                    postgres_types::ValuesRange::new(
                        Bound::Included(postgres_types::DbValue::Float4(2.23)),
                        Bound::Excluded(postgres_types::DbValue::Float4(4.55)),
                    ),
                )),
                postgres_types::DbValue::Range(postgres_types::Range::new(
                    "float4range".to_string(),
                    postgres_types::ValuesRange::new(
                        Bound::Included(postgres_types::DbValue::Float4(1.23)),
                        Bound::Unbounded,
                    ),
                )),
            ]),
            postgres_types::DbValue::Domain(postgres_types::Domain::new(
                "ddd".to_string(),
                postgres_types::DbValue::Varchar("tag2".to_string()),
            )),
            postgres_types::DbValue::Range(postgres_types::Range::new(
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
            )),
        ]
    }
}
