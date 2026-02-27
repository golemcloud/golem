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

// Guest binding version of `golem_wasm` crate's `IntoValueAndType` trait, to be upstreamed
// eventually.

pub mod golem_host;
pub mod http_uri;
pub mod tuples;
pub mod type_builder;
pub mod wasi;

#[cfg(test)]
mod test_macros;
#[cfg(test)]
mod tests;

#[cfg(feature = "bigdecimal")]
pub mod bigdecimal;
#[cfg(feature = "bit_vec")]
pub mod bit_vec;
#[cfg(feature = "bytes")]
pub mod bytes;
#[cfg(feature = "chrono")]
pub mod chrono;
#[cfg(feature = "mac_address")]
pub mod mac_address;
#[cfg(feature = "nonempty_collections")]
pub mod nonempty_collections;
#[cfg(feature = "num_bigint")]
pub mod num_bigint;
#[cfg(feature = "rust_decimal")]
pub mod rust_decimal;
#[cfg(feature = "serde_json_types")]
pub mod serde_json;
#[cfg(feature = "url")]
pub mod url;
pub mod uuid;

use crate::PromiseId;
use golem_wasm::golem_core_1_5_x::types::{NamedWitTypeNode, ResourceId, ValueAndType};
use golem_wasm::{
    AccountId, AgentId, ComponentId, NodeIndex, ResourceMode, Uri, Uuid, WitNode, WitType,
    WitTypeNode, WitValue, WitValueBuilderExtensions,
};
pub use golem_wasm::{NodeBuilder, WitValueExtractor};
use std::collections::Bound;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, LinkedList, VecDeque};
use std::hash::Hash;
use std::marker::PhantomData;
use std::net::IpAddr;
use std::num::{
    NonZeroI16, NonZeroI32, NonZeroI64, NonZeroI8, NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU8,
};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
pub use type_builder::TypeNodeBuilder;
use type_builder::WitTypeBuilderExtensions;

/// Specific trait to convert a type into a pair of `WitValue` and `WitType`.
pub trait IntoValue: Sized {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result;
    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result;

    fn into_value(self) -> WitValue {
        self.add_to_builder(WitValue::builder())
    }

    fn get_type() -> WitType {
        Self::add_to_type_builder(WitType::builder())
    }
}

pub trait IntoValueAndType {
    fn into_value_and_type(self) -> ValueAndType;
}

impl<T: IntoValue + Sized> IntoValueAndType for T {
    fn into_value_and_type(self) -> ValueAndType {
        ValueAndType {
            value: self.into_value(),
            typ: Self::get_type(),
        }
    }
}

pub trait FromValueAndType: Sized {
    fn from_value_and_type(value_and_type: ValueAndType) -> Result<Self, String> {
        Self::from_extractor(&value_and_type.value)
    }

    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String>;
}

impl IntoValue for u8 {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.u8(self)
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.u8()
    }
}

impl FromValueAndType for u8 {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor.u8().ok_or_else(|| "Expected u8".to_string())
    }
}

impl IntoValue for u16 {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.u16(self)
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.u16()
    }
}

impl FromValueAndType for u16 {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor.u16().ok_or_else(|| "Expected u16".to_string())
    }
}

impl IntoValue for u32 {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.u32(self)
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.u32()
    }
}

impl FromValueAndType for u32 {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor.u32().ok_or_else(|| "Expected u32".to_string())
    }
}

impl IntoValue for u64 {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.u64(self)
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.u64()
    }
}

impl FromValueAndType for u64 {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor.u64().ok_or_else(|| "Expected u64".to_string())
    }
}

impl IntoValue for usize {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.u64(self as u64)
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.u64()
    }
}

impl FromValueAndType for usize {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor
            .u64()
            .map(|n| n as usize)
            .ok_or_else(|| "Expected u64".to_string())
    }
}

impl IntoValue for i8 {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.s8(self)
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.s8()
    }
}

impl FromValueAndType for i8 {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor.s8().ok_or_else(|| "Expected i8".to_string())
    }
}

impl IntoValue for i16 {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.s16(self)
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.s16()
    }
}

impl FromValueAndType for i16 {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor.s16().ok_or_else(|| "Expected i16".to_string())
    }
}

impl IntoValue for i32 {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.s32(self)
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.s32()
    }
}

impl FromValueAndType for i32 {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor.s32().ok_or_else(|| "Expected i32".to_string())
    }
}

impl IntoValue for i64 {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.s64(self)
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.s64()
    }
}

impl FromValueAndType for i64 {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor.s64().ok_or_else(|| "Expected i64".to_string())
    }
}

impl IntoValue for f32 {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.f32(self)
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.f32()
    }
}

impl FromValueAndType for f32 {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor.f32().ok_or_else(|| "Expected f32".to_string())
    }
}

impl IntoValue for f64 {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.f64(self)
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.f64()
    }
}

impl FromValueAndType for f64 {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor.f64().ok_or_else(|| "Expected f64".to_string())
    }
}

impl IntoValue for bool {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.bool(self)
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.bool()
    }
}

impl FromValueAndType for bool {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor.bool().ok_or_else(|| "Expected bool".to_string())
    }
}

impl IntoValue for char {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.char(self)
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.char()
    }
}

impl FromValueAndType for char {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor.char().ok_or_else(|| "Expected char".to_string())
    }
}

impl IntoValue for String {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.string(&self)
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.string()
    }
}

impl FromValueAndType for String {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor
            .string()
            .map(|s| s.to_string())
            .ok_or_else(|| "Expected String".to_string())
    }
}

impl<S: IntoValue, E: IntoValue> IntoValue for Result<S, E> {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        match self {
            Ok(ok) => ok.add_to_builder(builder.result_ok()).finish(),
            Err(err) => err.add_to_builder(builder.result_err()).finish(),
        }
    }

    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        let mut builder = builder.result(None, None);
        builder = S::add_to_type_builder(builder.ok());
        builder = E::add_to_type_builder(builder.err());
        builder.finish()
    }
}

impl<S: FromValueAndType, E: FromValueAndType> FromValueAndType for Result<S, E> {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        match extractor.result() {
            Some(Ok(Some(ok))) => S::from_extractor(&ok).map(Ok),
            Some(Ok(None)) => Err("No value in Ok case".to_string()),
            Some(Err(Some(err))) => E::from_extractor(&err).map(Err),
            Some(Err(None)) => Err("No value in Err case".to_string()),
            None => Err("Expected Result".to_string()),
        }
    }
}

impl<E: IntoValue> IntoValue for Result<(), E> {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        match self {
            Ok(_) => builder.result_ok_unit(),
            Err(err) => err.add_to_builder(builder.result_err()).finish(),
        }
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.result(None, None);
        builder = builder.ok_unit();
        builder = E::add_to_type_builder(builder.err());
        builder.finish()
    }
}

impl<E: FromValueAndType> FromValueAndType for Result<(), E> {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        match extractor.result() {
            Some(Ok(Some(_))) => Err("Expected unit Ok case".to_string()),
            Some(Ok(None)) => Ok(Ok(())),
            Some(Err(Some(err))) => E::from_extractor(&err).map(Err),
            Some(Err(None)) => Err("No value in Err case".to_string()),
            None => Err("Expected Result".to_string()),
        }
    }
}

impl<S: IntoValue> IntoValue for Result<S, ()> {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        match self {
            Ok(ok) => ok.add_to_builder(builder.result_ok()).finish(),
            Err(_) => builder.result_err_unit(),
        }
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.result(None, None);
        builder = S::add_to_type_builder(builder.ok());
        builder = builder.err_unit();
        builder.finish()
    }
}

impl FromValueAndType for Result<(), ()> {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        match extractor.result() {
            Some(Ok(Some(_))) => Err("Expected unit Ok case".to_string()),
            Some(Ok(None)) => Ok(Ok(())),
            Some(Err(Some(_))) => Err("Expected unit Err case".to_string()),
            Some(Err(None)) => Ok(Err(())),
            None => Err("Expected Result".to_string()),
        }
    }
}

impl IntoValue for Result<(), ()> {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        match self {
            Ok(_) => builder.result_ok_unit(),
            Err(_) => builder.result_err_unit(),
        }
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.result(None, None);
        builder = builder.ok_unit();
        builder = builder.err_unit();
        builder.finish()
    }
}

impl<S: FromValueAndType> FromValueAndType for Result<S, ()> {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        match extractor.result() {
            Some(Ok(Some(ok))) => S::from_extractor(&ok).map(Ok),
            Some(Ok(None)) => Err("No value in Ok case".to_string()),
            Some(Err(Some(_))) => Err("Expected unit Err case".to_string()),
            Some(Err(None)) => Ok(Err(())),
            None => Err("Expected Result".to_string()),
        }
    }
}

impl<T: IntoValue> IntoValue for Option<T> {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        match self {
            Some(value) => value.add_to_builder(builder.option_some()).finish(),
            None => builder.option_none(),
        }
    }

    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        T::add_to_type_builder(builder.option(None, None)).finish()
    }
}

impl<T: FromValueAndType> FromValueAndType for Option<T> {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor
            .option()
            .ok_or_else(|| "Expected option".to_string())
            .and_then(|opt| {
                if let Some(value) = opt {
                    T::from_extractor(&value).map(Some)
                } else {
                    Ok(None)
                }
            })
    }
}

impl<T: IntoValue> IntoValue for Bound<T> {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        match self {
            Bound::Included(t) => {
                let builder = builder.variant(0);
                t.add_to_builder(builder).finish()
            }
            Bound::Excluded(t) => {
                let builder = builder.variant(1);
                t.add_to_builder(builder).finish()
            }
            Bound::Unbounded => builder.variant_unit(2),
        }
    }

    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        let mut builder = builder.variant(Some("Bound".to_string()), None);
        builder = T::add_to_type_builder(builder.case("included"));
        builder = T::add_to_type_builder(builder.case("excluded"));
        builder = builder.unit_case("unbounded");
        builder.finish()
    }
}

impl<T: FromValueAndType> FromValueAndType for Bound<T> {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        if let Some((case_idx, inner)) = extractor.variant() {
            match case_idx {
                0 => T::from_extractor(
                    &inner
                        .ok_or_else(|| "Missing variant value for inclusive bound".to_string())?,
                )
                .map(Bound::Included),
                1 => T::from_extractor(
                    &inner
                        .ok_or_else(|| "Missing variant value for exclusive bound".to_string())?,
                )
                .map(Bound::Excluded),
                2 => Ok(Bound::Unbounded),
                _ => Err(format!("Invalid variant case ({case_idx}) for Bound")),
            }
        } else {
            Err("Expected variant for Bound".to_string())
        }
    }
}

impl<T: IntoValue> IntoValue for Vec<T> {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        let mut list_builder = builder.list();
        for item in self {
            list_builder = item.add_to_builder(list_builder.item());
        }
        list_builder.finish()
    }

    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        T::add_to_type_builder(builder.list(None, None)).finish()
    }
}

impl<T: FromValueAndType> FromValueAndType for Vec<T> {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor
            .list_elements(|elem| T::from_extractor(&elem))
            .ok_or_else(|| "Expected list".to_string())
            .and_then(|list| list.into_iter().collect())
    }
}

impl<K: IntoValue, V: IntoValue> IntoValue for HashMap<K, V> {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let mut list_builder = builder.list();

        for (key, value) in self {
            let mut tuple_builder = list_builder.item().tuple();
            tuple_builder = key.add_to_builder(tuple_builder.item());
            tuple_builder = value.add_to_builder(tuple_builder.item());
            list_builder = tuple_builder.finish();
        }

        list_builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.list(None, None).tuple(None, None);
        builder = K::add_to_type_builder(builder.item());
        builder = V::add_to_type_builder(builder.item());
        builder.finish().finish()
    }
}

impl<K: FromValueAndType + Eq + Hash, V: FromValueAndType> FromValueAndType for HashMap<K, V> {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let items: Vec<(K, V)> = FromValueAndType::from_extractor(extractor)?;
        Ok(HashMap::from_iter(items))
    }
}

impl<T: IntoValue> IntoValue for Box<T> {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        (*self).add_to_builder(builder)
    }

    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        T::add_to_type_builder(builder)
    }
}

impl<T: FromValueAndType> FromValueAndType for Box<T> {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        T::from_extractor(extractor).map(Box::new)
    }
}

impl IntoValue for ComponentId {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        let builder = builder.record();
        let builder = self.uuid.add_to_builder(builder.item());
        builder.finish()
    }
    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        let builder = builder.record(Some("ComponentId".to_string()), None);
        let builder = <Uuid>::add_to_type_builder(builder.field("uuid"));
        builder.finish()
    }
}

impl FromValueAndType for ComponentId {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        Ok(Self {
            uuid: <Uuid>::from_extractor(
                &extractor
                    .field(0usize)
                    .ok_or_else(|| "Missing uuid field".to_string())?,
            )?,
        })
    }
}

impl IntoValue for AccountId {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        let builder = builder.record();
        let builder = self.uuid.add_to_builder(builder.item());
        builder.finish()
    }
    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        let builder = builder.record(Some("AccountId".to_string()), None);
        let builder = <Uuid>::add_to_type_builder(builder.field("uuid"));
        builder.finish()
    }
}

impl FromValueAndType for AccountId {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        Ok(Self {
            uuid: <Uuid>::from_extractor(
                &extractor
                    .field(0usize)
                    .ok_or_else(|| "Missing uuid field".to_string())?,
            )?,
        })
    }
}

impl IntoValue for Uuid {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        let builder = builder.record();
        let builder = self.high_bits.add_to_builder(builder.item());
        let builder = self.low_bits.add_to_builder(builder.item());
        builder.finish()
    }
    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        let builder = builder.record(Some("Uuid".to_string()), None);
        let builder = u64::add_to_type_builder(builder.field("high-bits"));
        let builder = u64::add_to_type_builder(builder.field("low-bits"));
        builder.finish()
    }
}
impl FromValueAndType for Uuid {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        Ok(Self {
            high_bits: u64::from_extractor(
                &extractor
                    .field(0usize)
                    .ok_or_else(|| "Missing high_bits field".to_string())?,
            )?,
            low_bits: u64::from_extractor(
                &extractor
                    .field(1usize)
                    .ok_or_else(|| "Missing low_bits field".to_string())?,
            )?,
        })
    }
}

impl IntoValue for ValueAndType {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        let builder = builder.record();
        let builder = self.value.add_to_builder(builder.item());
        let builder = self.typ.add_to_builder(builder.item());
        builder.finish()
    }
    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        let builder = builder.record(Some("ValueAndType".to_string()), None);
        let builder = <WitValue>::add_to_type_builder(builder.field("value"));
        let builder = <WitType>::add_to_type_builder(builder.field("typ"));
        builder.finish()
    }
}

impl FromValueAndType for ValueAndType {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        Ok(Self {
            value: <WitValue>::from_extractor(
                &extractor
                    .field(0usize)
                    .ok_or_else(|| "Missing value field".to_string())?,
            )?,
            typ: <WitType>::from_extractor(
                &extractor
                    .field(1usize)
                    .ok_or_else(|| "Missing typ field".to_string())?,
            )?,
        })
    }
}

impl IntoValue for WitValue {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        let builder = builder.record();
        let builder = self.nodes.add_to_builder(builder.item());
        builder.finish()
    }
    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        let builder = builder.record(Some("WitValue".to_string()), None);
        let builder = <Vec<WitNode>>::add_to_type_builder(builder.field("nodes"));
        builder.finish()
    }
}

impl FromValueAndType for WitValue {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        Ok(Self {
            nodes: <Vec<WitNode>>::from_extractor(
                &extractor
                    .field(0usize)
                    .ok_or_else(|| "Missing nodes field".to_string())?,
            )?,
        })
    }
}

impl IntoValue for WitNode {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        match self {
            WitNode::RecordValue(inner) => {
                let builder = builder.variant(0u32);
                inner.add_to_builder(builder).finish()
            }
            WitNode::VariantValue(inner) => {
                let builder = builder.variant(1u32);
                inner.add_to_builder(builder).finish()
            }
            WitNode::EnumValue(inner) => {
                let builder = builder.variant(2u32);
                inner.add_to_builder(builder).finish()
            }
            WitNode::FlagsValue(inner) => {
                let builder = builder.variant(3u32);
                inner.add_to_builder(builder).finish()
            }
            WitNode::TupleValue(inner) => {
                let builder = builder.variant(4u32);
                inner.add_to_builder(builder).finish()
            }
            WitNode::ListValue(inner) => {
                let builder = builder.variant(5u32);
                inner.add_to_builder(builder).finish()
            }
            WitNode::OptionValue(inner) => {
                let builder = builder.variant(6u32);
                inner.add_to_builder(builder).finish()
            }
            WitNode::ResultValue(inner) => {
                let builder = builder.variant(7u32);
                inner.add_to_builder(builder).finish()
            }
            WitNode::PrimU8(inner) => {
                let builder = builder.variant(8u32);
                inner.add_to_builder(builder).finish()
            }
            WitNode::PrimU16(inner) => {
                let builder = builder.variant(9u32);
                inner.add_to_builder(builder).finish()
            }
            WitNode::PrimU32(inner) => {
                let builder = builder.variant(10u32);
                inner.add_to_builder(builder).finish()
            }
            WitNode::PrimU64(inner) => {
                let builder = builder.variant(11u32);
                inner.add_to_builder(builder).finish()
            }
            WitNode::PrimS8(inner) => {
                let builder = builder.variant(12u32);
                inner.add_to_builder(builder).finish()
            }
            WitNode::PrimS16(inner) => {
                let builder = builder.variant(13u32);
                inner.add_to_builder(builder).finish()
            }
            WitNode::PrimS32(inner) => {
                let builder = builder.variant(14u32);
                inner.add_to_builder(builder).finish()
            }
            WitNode::PrimS64(inner) => {
                let builder = builder.variant(15u32);
                inner.add_to_builder(builder).finish()
            }
            WitNode::PrimFloat32(inner) => {
                let builder = builder.variant(16u32);
                inner.add_to_builder(builder).finish()
            }
            WitNode::PrimFloat64(inner) => {
                let builder = builder.variant(17u32);
                inner.add_to_builder(builder).finish()
            }
            WitNode::PrimChar(inner) => {
                let builder = builder.variant(18u32);
                inner.add_to_builder(builder).finish()
            }
            WitNode::PrimBool(inner) => {
                let builder = builder.variant(19u32);
                inner.add_to_builder(builder).finish()
            }
            WitNode::PrimString(inner) => {
                let builder = builder.variant(20u32);
                inner.add_to_builder(builder).finish()
            }
            WitNode::Handle(inner) => {
                let builder = builder.variant(21u32);
                inner.add_to_builder(builder).finish()
            }
        }
    }
    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        let builder = builder.variant(Some("WitNode".to_string()), None);
        let builder = <Vec<NodeIndex>>::add_to_type_builder(builder.case("record-value"));
        let builder =
            <(u32, Option<NodeIndex>)>::add_to_type_builder(builder.case("variant-value"));
        let builder = <u32>::add_to_type_builder(builder.case("enum-value"));
        let builder = <Vec<bool>>::add_to_type_builder(builder.case("flags-value"));
        let builder = <Vec<NodeIndex>>::add_to_type_builder(builder.case("tuple-value"));
        let builder = <Vec<NodeIndex>>::add_to_type_builder(builder.case("list-value"));
        let builder = <Option<NodeIndex>>::add_to_type_builder(builder.case("option-value"));
        let builder = <Result<Option<NodeIndex>, Option<NodeIndex>>>::add_to_type_builder(
            builder.case("result-value"),
        );
        let builder = u8::add_to_type_builder(builder.case("prim-u8"));
        let builder = u16::add_to_type_builder(builder.case("prim-u16"));
        let builder = u32::add_to_type_builder(builder.case("prim-u32"));
        let builder = u64::add_to_type_builder(builder.case("prim-u64"));
        let builder = i8::add_to_type_builder(builder.case("prim-s8"));
        let builder = i16::add_to_type_builder(builder.case("prim-s16"));
        let builder = i32::add_to_type_builder(builder.case("prim-s32"));
        let builder = i64::add_to_type_builder(builder.case("prim-s64"));
        let builder = f32::add_to_type_builder(builder.case("prim-float32"));
        let builder = f64::add_to_type_builder(builder.case("prim-float64"));
        let builder = char::add_to_type_builder(builder.case("prim-char"));
        let builder = bool::add_to_type_builder(builder.case("prim-bool"));
        let builder = String::add_to_type_builder(builder.case("prim-string"));
        let builder = <(Uri, u64)>::add_to_type_builder(builder.case("handle"));
        builder.finish()
    }
}
impl FromValueAndType for WitNode {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let (idx, inner) = extractor
            .variant()
            .ok_or_else(|| "WitNode should be variant".to_string())?;
        match idx {
            0 => Ok(WitNode::RecordValue(<Vec<NodeIndex>>::from_extractor(
                &inner.ok_or_else(|| "Missing RecordValue body".to_string())?,
            )?)),
            1 => Ok(WitNode::VariantValue(
                <(u32, Option<NodeIndex>)>::from_extractor(
                    &inner.ok_or_else(|| "Missing VariantValue body".to_string())?,
                )?,
            )),
            2 => Ok(WitNode::EnumValue(<u32>::from_extractor(
                &inner.ok_or_else(|| "Missing EnumValue body".to_string())?,
            )?)),
            3 => Ok(WitNode::FlagsValue(<Vec<bool>>::from_extractor(
                &inner.ok_or_else(|| "Missing FlagsValue body".to_string())?,
            )?)),
            4 => Ok(WitNode::TupleValue(<Vec<NodeIndex>>::from_extractor(
                &inner.ok_or_else(|| "Missing TupleValue body".to_string())?,
            )?)),
            5 => Ok(WitNode::ListValue(<Vec<NodeIndex>>::from_extractor(
                &inner.ok_or_else(|| "Missing ListValue body".to_string())?,
            )?)),
            6 => Ok(WitNode::OptionValue(<Option<NodeIndex>>::from_extractor(
                &inner.ok_or_else(|| "Missing OptionValue body".to_string())?,
            )?)),
            7 => Ok(WitNode::ResultValue(<Result<
                Option<NodeIndex>,
                Option<NodeIndex>,
            >>::from_extractor(
                &inner.ok_or_else(|| "Missing ResultValue body".to_string())?,
            )?)),
            8 => Ok(WitNode::PrimU8(u8::from_extractor(
                &inner.ok_or_else(|| "Missing PrimU8 body".to_string())?,
            )?)),
            9 => Ok(WitNode::PrimU16(u16::from_extractor(
                &inner.ok_or_else(|| "Missing PrimU16 body".to_string())?,
            )?)),
            10 => Ok(WitNode::PrimU32(u32::from_extractor(
                &inner.ok_or_else(|| "Missing PrimU32 body".to_string())?,
            )?)),
            11 => Ok(WitNode::PrimU64(u64::from_extractor(
                &inner.ok_or_else(|| "Missing PrimU64 body".to_string())?,
            )?)),
            12 => Ok(WitNode::PrimS8(i8::from_extractor(
                &inner.ok_or_else(|| "Missing PrimS8 body".to_string())?,
            )?)),
            13 => Ok(WitNode::PrimS16(i16::from_extractor(
                &inner.ok_or_else(|| "Missing PrimS16 body".to_string())?,
            )?)),
            14 => Ok(WitNode::PrimS32(i32::from_extractor(
                &inner.ok_or_else(|| "Missing PrimS32 body".to_string())?,
            )?)),
            15 => Ok(WitNode::PrimS64(i64::from_extractor(
                &inner.ok_or_else(|| "Missing PrimS64 body".to_string())?,
            )?)),
            16 => Ok(WitNode::PrimFloat32(<f32>::from_extractor(
                &inner.ok_or_else(|| "Missing PrimFloat32 body".to_string())?,
            )?)),
            17 => Ok(WitNode::PrimFloat64(<f64>::from_extractor(
                &inner.ok_or_else(|| "Missing PrimFloat64 body".to_string())?,
            )?)),
            18 => Ok(WitNode::PrimChar(<char>::from_extractor(
                &inner.ok_or_else(|| "Missing PrimChar body".to_string())?,
            )?)),
            19 => Ok(WitNode::PrimBool(<bool>::from_extractor(
                &inner.ok_or_else(|| "Missing PrimBool body".to_string())?,
            )?)),
            20 => Ok(WitNode::PrimString(<String>::from_extractor(
                &inner.ok_or_else(|| "Missing PrimString body".to_string())?,
            )?)),
            21 => Ok(WitNode::Handle(<(Uri, u64)>::from_extractor(
                &inner.ok_or_else(|| "Missing Handle body".to_string())?,
            )?)),
            _ => Err(format!("Invalid WitNode variant: {}", idx)),
        }
    }
}

impl IntoValue for Uri {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        let builder = builder.record();
        let builder = self.value.add_to_builder(builder.item());
        builder.finish()
    }
    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        let builder = builder.record(Some("Uri".to_string()), None);
        let builder = String::add_to_type_builder(builder.field("value"));
        builder.finish()
    }
}
impl FromValueAndType for Uri {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        Ok(Self {
            value: String::from_extractor(
                &extractor
                    .field(0usize)
                    .ok_or_else(|| "Missing value field".to_string())?,
            )?,
        })
    }
}

// Wit-Type
impl IntoValue for WitType {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        let builder = builder.record();
        let builder = self.nodes.add_to_builder(builder.item());
        builder.finish()
    }
    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        let builder = builder.record(Some("WitType".to_string()), None);
        let builder = <Vec<NamedWitTypeNode>>::add_to_type_builder(builder.field("nodes"));
        builder.finish()
    }
}

impl FromValueAndType for WitType {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        Ok(Self {
            nodes: <Vec<NamedWitTypeNode>>::from_extractor(
                &extractor
                    .field(0usize)
                    .ok_or_else(|| "Missing nodes field".to_string())?,
            )?,
        })
    }
}

impl IntoValue for NamedWitTypeNode {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        let builder = builder.record();
        let builder = self.name.add_to_builder(builder.item());
        let builder = self.owner.add_to_builder(builder.item());
        let builder = self.type_.add_to_builder(builder.item());
        builder.finish()
    }
    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        let builder = builder.record(Some("NamedWitTypeNode".to_string()), None);
        let builder = <Option<String>>::add_to_type_builder(builder.field("name"));
        let builder = <Option<String>>::add_to_type_builder(builder.field("owner"));
        let builder = WitTypeNode::add_to_type_builder(builder.field("type"));
        builder.finish()
    }
}

impl FromValueAndType for NamedWitTypeNode {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        Ok(Self {
            name: <Option<String>>::from_extractor(
                &extractor
                    .field(0usize)
                    .ok_or_else(|| "Missing name field".to_string())?,
            )?,
            owner: <Option<String>>::from_extractor(
                &extractor
                    .field(1usize)
                    .ok_or_else(|| "Missing owner field".to_string())?,
            )?,
            type_: WitTypeNode::from_extractor(
                &extractor
                    .field(2usize)
                    .ok_or_else(|| "Missing type_ field".to_string())?,
            )?,
        })
    }
}

impl IntoValue for WitTypeNode {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        match self {
            WitTypeNode::RecordType(inner) => {
                let builder = builder.variant(0);
                inner.add_to_builder(builder).finish()
            }
            WitTypeNode::VariantType(inner) => {
                let builder = builder.variant(1);
                inner.add_to_builder(builder).finish()
            }
            WitTypeNode::EnumType(inner) => {
                let builder = builder.variant(2);
                inner.add_to_builder(builder).finish()
            }
            WitTypeNode::FlagsType(inner) => {
                let builder = builder.variant(3);
                inner.add_to_builder(builder).finish()
            }
            WitTypeNode::TupleType(inner) => {
                let builder = builder.variant(4);
                inner.add_to_builder(builder).finish()
            }
            WitTypeNode::ListType(inner) => {
                let builder = builder.variant(5);
                inner.add_to_builder(builder).finish()
            }
            WitTypeNode::OptionType(inner) => {
                let builder = builder.variant(6);
                inner.add_to_builder(builder).finish()
            }
            WitTypeNode::ResultType(inner) => {
                let builder = builder.variant(7);
                inner.add_to_builder(builder).finish()
            }
            WitTypeNode::PrimU8Type => builder.variant_unit(8),
            WitTypeNode::PrimU16Type => builder.variant_unit(9),
            WitTypeNode::PrimU32Type => builder.variant_unit(10),
            WitTypeNode::PrimU64Type => builder.variant_unit(11),
            WitTypeNode::PrimS8Type => builder.variant_unit(12),
            WitTypeNode::PrimS16Type => builder.variant_unit(13),
            WitTypeNode::PrimS32Type => builder.variant_unit(14),
            WitTypeNode::PrimS64Type => builder.variant_unit(15),
            WitTypeNode::PrimF32Type => builder.variant_unit(16),
            WitTypeNode::PrimF64Type => builder.variant_unit(17),
            WitTypeNode::PrimCharType => builder.variant_unit(18),
            WitTypeNode::PrimBoolType => builder.variant_unit(19),
            WitTypeNode::PrimStringType => builder.variant_unit(20),
            WitTypeNode::HandleType(inner) => {
                let builder = builder.variant(21);
                inner.add_to_builder(builder).finish()
            }
        }
    }
    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        let builder = builder.variant(Some("WitTypeNode".to_string()), None);
        let builder = <Vec<(String, NodeIndex)>>::add_to_type_builder(builder.case("record-type"));
        let builder =
            <Vec<(String, Option<NodeIndex>)>>::add_to_type_builder(builder.case("variant-type"));
        let builder = <Vec<String>>::add_to_type_builder(builder.case("enum-type"));
        let builder = <Vec<String>>::add_to_type_builder(builder.case("flags-type"));
        let builder = <Vec<NodeIndex>>::add_to_type_builder(builder.case("tuple-type"));
        let builder = <NodeIndex>::add_to_type_builder(builder.case("list-type"));
        let builder = <NodeIndex>::add_to_type_builder(builder.case("option-type"));
        let builder = <(Option<NodeIndex>, Option<NodeIndex>)>::add_to_type_builder(
            builder.case("result-type"),
        );
        let builder = builder.unit_case("prim-u8-type");
        let builder = builder.unit_case("prim-u16-type");
        let builder = builder.unit_case("prim-u32-type");
        let builder = builder.unit_case("prim-u64-type");
        let builder = builder.unit_case("prim-s8-type");
        let builder = builder.unit_case("prim-s16-type");
        let builder = builder.unit_case("prim-s32-type");
        let builder = builder.unit_case("prim-s64-type");
        let builder = builder.unit_case("prim-f32-type");
        let builder = builder.unit_case("prim-f64-type");
        let builder = builder.unit_case("prim-char-type");
        let builder = builder.unit_case("prim-bool-type");
        let builder = builder.unit_case("prim-string-type");
        let builder =
            <(ResourceId, ResourceMode)>::add_to_type_builder(builder.case("handle-type"));
        builder.finish()
    }
}

impl FromValueAndType for WitTypeNode {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let (idx, inner) = extractor
            .variant()
            .ok_or_else(|| "WitTypeNode should be variant".to_string())?;
        match idx {
            0u32 => Ok(WitTypeNode::RecordType(
                <Vec<(String, NodeIndex)>>::from_extractor(
                    &inner.ok_or_else(|| "Missing RecordType body".to_string())?,
                )?,
            )),
            1u32 => Ok(WitTypeNode::VariantType(
                <Vec<(String, Option<NodeIndex>)>>::from_extractor(
                    &inner.ok_or_else(|| "Missing VariantType body".to_string())?,
                )?,
            )),
            2u32 => Ok(WitTypeNode::EnumType(<Vec<String>>::from_extractor(
                &inner.ok_or_else(|| "Missing EnumType body".to_string())?,
            )?)),
            3u32 => Ok(WitTypeNode::FlagsType(<Vec<String>>::from_extractor(
                &inner.ok_or_else(|| "Missing FlagsType body".to_string())?,
            )?)),
            4u32 => Ok(WitTypeNode::TupleType(<Vec<NodeIndex>>::from_extractor(
                &inner.ok_or_else(|| "Missing TupleType body".to_string())?,
            )?)),
            5u32 => Ok(WitTypeNode::ListType(<NodeIndex>::from_extractor(
                &inner.ok_or_else(|| "Missing ListType body".to_string())?,
            )?)),
            6u32 => Ok(WitTypeNode::OptionType(<NodeIndex>::from_extractor(
                &inner.ok_or_else(|| "Missing OptionType body".to_string())?,
            )?)),
            7u32 => Ok(WitTypeNode::ResultType(<(
                Option<NodeIndex>,
                Option<NodeIndex>,
            )>::from_extractor(
                &inner.ok_or_else(|| "Missing ResultType body".to_string())?,
            )?)),
            8u32 => Ok(WitTypeNode::PrimU8Type),
            9u32 => Ok(WitTypeNode::PrimU16Type),
            10u32 => Ok(WitTypeNode::PrimU32Type),
            11u32 => Ok(WitTypeNode::PrimU64Type),
            12u32 => Ok(WitTypeNode::PrimS8Type),
            13u32 => Ok(WitTypeNode::PrimS16Type),
            14u32 => Ok(WitTypeNode::PrimS32Type),
            15u32 => Ok(WitTypeNode::PrimS64Type),
            16u32 => Ok(WitTypeNode::PrimF32Type),
            17u32 => Ok(WitTypeNode::PrimF64Type),
            18u32 => Ok(WitTypeNode::PrimCharType),
            19u32 => Ok(WitTypeNode::PrimBoolType),
            20u32 => Ok(WitTypeNode::PrimStringType),
            21u32 => Ok(WitTypeNode::HandleType(
                <(ResourceId, ResourceMode)>::from_extractor(
                    &inner.ok_or_else(|| "Missing HandleType body".to_string())?,
                )?,
            )),
            _ => Err(format!("Invalid wit-type node variant {}", idx)),
        }
    }
}

impl IntoValue for ResourceMode {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        match self {
            ResourceMode::Owned => builder.enum_value(0u32),
            ResourceMode::Borrowed => builder.enum_value(1u32),
        }
    }
    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        builder.r#enum(
            Some("ResourceMode".to_string()),
            None,
            &["owned", "borrowed"],
        )
    }
}

impl FromValueAndType for ResourceMode {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        match extractor.enum_value() {
            Some(0) => Ok(ResourceMode::Owned),
            Some(1) => Ok(ResourceMode::Borrowed),
            _ => Err("Invalid ResourceMode".to_string()),
        }
    }
}

// &str implementation - treat as String
impl IntoValue for &str {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.string(self)
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.string()
    }
}

// HashSet<T> implementation - treat as list
impl<T: IntoValue + Eq + Hash> IntoValue for HashSet<T> {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        let mut list_builder = builder.list();
        for item in self {
            list_builder = item.add_to_builder(list_builder.item());
        }
        list_builder.finish()
    }

    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        T::add_to_type_builder(builder.list(None, None)).finish()
    }
}

impl<T: FromValueAndType + Eq + Hash> FromValueAndType for HashSet<T> {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let items: Vec<T> = FromValueAndType::from_extractor(extractor)?;
        Ok(HashSet::from_iter(items))
    }
}

// BTreeMap<K, V> implementation - treat as list of tuples
impl<K: IntoValue + Ord, V: IntoValue> IntoValue for BTreeMap<K, V> {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let mut list_builder = builder.list();

        for (key, value) in self {
            let mut tuple_builder = list_builder.item().tuple();
            tuple_builder = key.add_to_builder(tuple_builder.item());
            tuple_builder = value.add_to_builder(tuple_builder.item());
            list_builder = tuple_builder.finish();
        }

        list_builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.list(None, None).tuple(None, None);
        builder = K::add_to_type_builder(builder.item());
        builder = V::add_to_type_builder(builder.item());
        builder.finish().finish()
    }
}

impl<K: FromValueAndType + Ord, V: FromValueAndType> FromValueAndType for BTreeMap<K, V> {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let items: Vec<(K, V)> = FromValueAndType::from_extractor(extractor)?;
        Ok(BTreeMap::from_iter(items))
    }
}

// BTreeSet<T> implementation - treat as list
impl<T: IntoValue + Ord> IntoValue for BTreeSet<T> {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        let mut list_builder = builder.list();
        for item in self {
            list_builder = item.add_to_builder(list_builder.item());
        }
        list_builder.finish()
    }

    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        T::add_to_type_builder(builder.list(None, None)).finish()
    }
}

impl<T: FromValueAndType + Ord> FromValueAndType for BTreeSet<T> {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let items: Vec<T> = FromValueAndType::from_extractor(extractor)?;
        Ok(BTreeSet::from_iter(items))
    }
}

// VecDeque<T> implementation - treat as list
impl<T: IntoValue> IntoValue for VecDeque<T> {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        let mut list_builder = builder.list();
        for item in self {
            list_builder = item.add_to_builder(list_builder.item());
        }
        list_builder.finish()
    }

    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        T::add_to_type_builder(builder.list(None, None)).finish()
    }
}

impl<T: FromValueAndType> FromValueAndType for VecDeque<T> {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let items: Vec<T> = FromValueAndType::from_extractor(extractor)?;
        Ok(VecDeque::from_iter(items))
    }
}

// LinkedList<T> implementation - treat as list
impl<T: IntoValue> IntoValue for LinkedList<T> {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        let mut list_builder = builder.list();
        for item in self {
            list_builder = item.add_to_builder(list_builder.item());
        }
        list_builder.finish()
    }

    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        T::add_to_type_builder(builder.list(None, None)).finish()
    }
}

impl<T: FromValueAndType> FromValueAndType for LinkedList<T> {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let items: Vec<T> = FromValueAndType::from_extractor(extractor)?;
        Ok(LinkedList::from_iter(items))
    }
}

// NonZero unsigned integers - treat as their underlying unsigned type
impl IntoValue for NonZeroU8 {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.u8(self.get())
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.u8()
    }
}

impl FromValueAndType for NonZeroU8 {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor
            .u8()
            .and_then(NonZeroU8::new)
            .ok_or_else(|| "Expected non-zero u8".to_string())
    }
}

impl IntoValue for NonZeroU16 {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.u16(self.get())
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.u16()
    }
}

impl FromValueAndType for NonZeroU16 {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor
            .u16()
            .and_then(NonZeroU16::new)
            .ok_or_else(|| "Expected non-zero u16".to_string())
    }
}

impl IntoValue for NonZeroU32 {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.u32(self.get())
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.u32()
    }
}

impl FromValueAndType for NonZeroU32 {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor
            .u32()
            .and_then(NonZeroU32::new)
            .ok_or_else(|| "Expected non-zero u32".to_string())
    }
}

impl IntoValue for NonZeroU64 {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.u64(self.get())
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.u64()
    }
}

impl FromValueAndType for NonZeroU64 {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor
            .u64()
            .and_then(NonZeroU64::new)
            .ok_or_else(|| "Expected non-zero u64".to_string())
    }
}

// NonZero signed integers - treat as their underlying signed type
impl IntoValue for NonZeroI8 {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.s8(self.get())
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.s8()
    }
}

impl FromValueAndType for NonZeroI8 {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor
            .s8()
            .and_then(NonZeroI8::new)
            .ok_or_else(|| "Expected non-zero i8".to_string())
    }
}

impl IntoValue for NonZeroI16 {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.s16(self.get())
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.s16()
    }
}

impl FromValueAndType for NonZeroI16 {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor
            .s16()
            .and_then(NonZeroI16::new)
            .ok_or_else(|| "Expected non-zero i16".to_string())
    }
}

impl IntoValue for NonZeroI32 {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.s32(self.get())
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.s32()
    }
}

impl FromValueAndType for NonZeroI32 {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor
            .s32()
            .and_then(NonZeroI32::new)
            .ok_or_else(|| "Expected non-zero i32".to_string())
    }
}

impl IntoValue for NonZeroI64 {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        builder.s64(self.get())
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        builder.s64()
    }
}

impl FromValueAndType for NonZeroI64 {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        extractor
            .s64()
            .and_then(NonZeroI64::new)
            .ok_or_else(|| "Expected non-zero i64".to_string())
    }
}

// Duration implementation - treat as record with seconds and nanos
impl IntoValue for Duration {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        let builder = builder.record();
        let builder = self.as_secs().add_to_builder(builder.item());
        let builder = self.subsec_nanos().add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        let builder = builder.record(Some("Duration".to_string()), None);
        let builder = u64::add_to_type_builder(builder.field("secs"));
        let builder = u32::add_to_type_builder(builder.field("nanos"));
        builder.finish()
    }
}

impl FromValueAndType for Duration {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let secs = u64::from_extractor(
            &extractor
                .field(0usize)
                .ok_or_else(|| "Missing secs field".to_string())?,
        )?;
        let nanos = u32::from_extractor(
            &extractor
                .field(1usize)
                .ok_or_else(|| "Missing nanos field".to_string())?,
        )?;
        Ok(Duration::new(secs, nanos))
    }
}

// Range<T> implementation - treat as record with start and end
impl<T: IntoValue> IntoValue for Range<T> {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        let builder = builder.record();
        let builder = self.start.add_to_builder(builder.item());
        let builder = self.end.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        let builder = builder.record(Some("Range".to_string()), None);
        let builder = T::add_to_type_builder(builder.field("start"));
        let builder = T::add_to_type_builder(builder.field("end"));
        builder.finish()
    }
}

impl<T: FromValueAndType> FromValueAndType for Range<T> {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let start = T::from_extractor(
            &extractor
                .field(0usize)
                .ok_or_else(|| "Missing start field".to_string())?,
        )?;
        let end = T::from_extractor(
            &extractor
                .field(1usize)
                .ok_or_else(|| "Missing end field".to_string())?,
        )?;
        Ok(Range { start, end })
    }
}

// Rc<T> implementation - treat as inner type
impl<T: IntoValue + Clone> IntoValue for Rc<T> {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        (*self).clone().add_to_builder(builder)
    }

    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        T::add_to_type_builder(builder)
    }
}

impl<T: FromValueAndType> FromValueAndType for Rc<T> {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        T::from_extractor(extractor).map(Rc::new)
    }
}

// Arc<T> implementation - treat as inner type
impl<T: IntoValue + Clone> IntoValue for Arc<T> {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        (*self).clone().add_to_builder(builder)
    }

    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        T::add_to_type_builder(builder)
    }
}

impl<T: FromValueAndType> FromValueAndType for Arc<T> {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        T::from_extractor(extractor).map(Arc::new)
    }
}

// PhantomData<T> implementation - treat as empty record (always empty)
impl<T> IntoValue for PhantomData<T> {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        builder.record().finish()
    }

    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        builder
            .record(Some("PhantomData".to_string()), None)
            .finish()
    }
}

impl<T> FromValueAndType for PhantomData<T> {
    fn from_extractor<'a, 'b>(
        _extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        // PhantomData is always empty, so just consume nothing
        Ok(PhantomData)
    }
}

// IpAddr implementation - treat as enum variant
impl IntoValue for IpAddr {
    fn add_to_builder<B: NodeBuilder>(self, builder: B) -> B::Result {
        match self {
            IpAddr::V4(addr) => {
                let builder = builder.variant(0);
                addr.octets().to_vec().add_to_builder(builder).finish()
            }
            IpAddr::V6(addr) => {
                let builder = builder.variant(1);
                addr.segments().to_vec().add_to_builder(builder).finish()
            }
        }
    }

    fn add_to_type_builder<B: TypeNodeBuilder>(builder: B) -> B::Result {
        let mut builder = builder.variant(Some("IpAddr".to_string()), None);
        builder = <Vec<u8>>::add_to_type_builder(builder.case("v4"));
        builder = <Vec<u16>>::add_to_type_builder(builder.case("v6"));
        builder.finish()
    }
}

impl FromValueAndType for IpAddr {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let (idx, inner) = extractor
            .variant()
            .ok_or_else(|| "IpAddr should be variant".to_string())?;
        match idx {
            0 => {
                let octets: Vec<u8> = FromValueAndType::from_extractor(
                    &inner.ok_or_else(|| "Missing IpAddrV4 body".to_string())?,
                )?;
                if octets.len() != 4 {
                    return Err("IPv4 address must have 4 octets".to_string());
                }
                Ok(IpAddr::V4(std::net::Ipv4Addr::new(
                    octets[0], octets[1], octets[2], octets[3],
                )))
            }
            1 => {
                let segments: Vec<u16> = FromValueAndType::from_extractor(
                    &inner.ok_or_else(|| "Missing IpAddrV6 body".to_string())?,
                )?;
                if segments.len() != 8 {
                    return Err("IPv6 address must have 8 segments".to_string());
                }
                Ok(IpAddr::V6(std::net::Ipv6Addr::new(
                    segments[0],
                    segments[1],
                    segments[2],
                    segments[3],
                    segments[4],
                    segments[5],
                    segments[6],
                    segments[7],
                )))
            }
            _ => Err(format!("Invalid IpAddr variant: {}", idx)),
        }
    }
}

impl IntoValue for AgentId {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = self.component_id.add_to_builder(builder.item());
        let builder = self.agent_id.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(Some("agent-id".to_string()), Some("golem".to_string()));
        let builder = ComponentId::add_to_type_builder(builder.field("component-id"));
        let builder = String::add_to_type_builder(builder.field("agent-id"));
        builder.finish()
    }
}

impl FromValueAndType for AgentId {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let component_id = ComponentId::from_extractor(
            &extractor
                .field(0)
                .ok_or_else(|| "Missing component-id field".to_string())?,
        )?;
        let agent_id = String::from_extractor(
            &extractor
                .field(1)
                .ok_or_else(|| "Missing agent-id field".to_string())?,
        )?;
        Ok(AgentId {
            component_id,
            agent_id,
        })
    }
}

impl IntoValue for PromiseId {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = self.agent_id.add_to_builder(builder.item());
        let builder = self.oplog_idx.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(Some("promise-id".to_string()), Some("golem".to_string()));
        let builder = AgentId::add_to_type_builder(builder.field("agent-id"));
        let builder = u64::add_to_type_builder(builder.field("oplog-idx"));
        builder.finish()
    }
}

impl FromValueAndType for PromiseId {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let agent_id = AgentId::from_extractor(
            &extractor
                .field(0)
                .ok_or_else(|| "Missing agent-id field".to_string())?,
        )?;
        let oplog_idx = u64::from_extractor(
            &extractor
                .field(1)
                .ok_or_else(|| "Missing oplog-idx field".to_string())?,
        )?;
        Ok(PromiseId {
            agent_id,
            oplog_idx,
        })
    }
}
