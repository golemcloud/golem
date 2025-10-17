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

// Guest binding version of `golem_wasm_rpc` crate's `IntoValueAndType` trait, to be upstreamed
// eventually.

pub mod type_builder;

use crate::value_and_type::type_builder::WitTypeBuilderExtensions;
use golem_wasm_rpc::golem_rpc_0_2_x::types::ValueAndType;
use golem_wasm_rpc::{WitType, WitValue, WitValueBuilderExtensions};
use std::collections::Bound;
use std::collections::HashMap;
use std::hash::Hash;

pub use golem_wasm_rpc::{NodeBuilder, WitValueExtractor};
pub use type_builder::TypeNodeBuilder;

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
            Ok(_) => builder.result_ok().finish().finish(),
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
            Err(_) => builder.result_err().finish().finish(),
        }
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.result(None, None);
        builder = S::add_to_type_builder(builder.ok());
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
            Some(Err(Some(_))) => Ok(Err(())),
            Some(Err(None)) => Err("Expected unit Err case".to_string()),
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

impl<A: IntoValue, B: IntoValue> IntoValue for (A, B) {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let mut builder = builder.tuple();
        builder = self.0.add_to_builder(builder.item());
        builder = self.1.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.tuple(None, None);
        builder = A::add_to_type_builder(builder.item());
        builder = B::add_to_type_builder(builder.item());
        builder.finish()
    }
}

impl<A: FromValueAndType, B: FromValueAndType> FromValueAndType for (A, B) {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let a = A::from_extractor(
            &extractor
                .tuple_element(0)
                .ok_or_else(|| "Expected 2-tuple".to_string())?,
        )?;
        let b = B::from_extractor(
            &extractor
                .tuple_element(1)
                .ok_or_else(|| "Expected 2-tuple".to_string())?,
        )?;
        Ok((a, b))
    }
}

impl<A: IntoValue, B: IntoValue, C: IntoValue> IntoValue for (A, B, C) {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let mut builder = builder.tuple();
        builder = self.0.add_to_builder(builder.item());
        builder = self.1.add_to_builder(builder.item());
        builder = self.2.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.tuple(None, None);
        builder = A::add_to_type_builder(builder.item());
        builder = B::add_to_type_builder(builder.item());
        builder = C::add_to_type_builder(builder.item());
        builder.finish()
    }
}

impl<A: FromValueAndType, B: FromValueAndType, C: FromValueAndType> FromValueAndType for (A, B, C) {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let a = A::from_extractor(
            &extractor
                .tuple_element(0)
                .ok_or_else(|| "Expected 3-tuple".to_string())?,
        )?;
        let b = B::from_extractor(
            &extractor
                .tuple_element(1)
                .ok_or_else(|| "Expected 3-tuple".to_string())?,
        )?;
        let c = C::from_extractor(
            &extractor
                .tuple_element(2)
                .ok_or_else(|| "Expected 3-tuple".to_string())?,
        )?;
        Ok((a, b, c))
    }
}

impl<K: IntoValue, V: IntoValue> IntoValue for HashMap<K, V> {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let mut builder = builder.list();
        for (k, v) in self {
            builder = k.add_to_builder(builder.item().tuple().item()).finish();
            builder = v.add_to_builder(builder.item().tuple().item()).finish();
        }
        builder.finish()
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
