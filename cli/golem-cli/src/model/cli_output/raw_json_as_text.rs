// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

//! A serializer that writes each raw JSON value (`serde_json::value::RawValue`) as a string that
//! holds its exact text, and gives everything else to the serializer that it wraps.
//!
//! The CLI makes a `serde_json::Value` of its structured output before it prints JSON, YAML or
//! TOON. A `Value` cannot hold every raw JSON value: the conversion parses the text again, so it
//! rounds a number that no `f64` holds exactly, refuses a number beyond the `f64` range, and
//! refuses nesting deeper than 128. A string holds the exact text, and every format prints a
//! string as it is.

use serde::Serialize;
use serde::ser::{self, Error as _, Serializer};

/// The struct name with which `serde_json` marks a raw JSON value when it serializes one.
const RAW_VALUE_TOKEN: &str = "$serde_json::private::RawValue";

/// Wraps a serializer. A raw JSON value becomes a string with its exact text; every other value
/// goes to the wrapped serializer, and so do the values inside it.
pub struct RawJsonAsText<S>(pub S);

/// A value whose parts also go through [`RawJsonAsText`].
struct Wrapped<'a, T: ?Sized>(&'a T);

impl<T: Serialize + ?Sized> Serialize for Wrapped<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(RawJsonAsText(serializer))
    }
}

/// A sequence, tuple, map or struct variant of the wrapped serializer.
pub struct Compound<C>(C);

/// A struct of the wrapped serializer, or the text of a raw JSON value.
pub enum StructOrRawText<S: Serializer> {
    Struct(S::SerializeStruct),
    RawText { serializer: S, text: Option<String> },
}

impl<S: Serializer> Serializer for RawJsonAsText<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = Compound<S::SerializeSeq>;
    type SerializeTuple = Compound<S::SerializeTuple>;
    type SerializeTupleStruct = Compound<S::SerializeTupleStruct>;
    type SerializeTupleVariant = Compound<S::SerializeTupleVariant>;
    type SerializeMap = Compound<S::SerializeMap>;
    type SerializeStruct = StructOrRawText<S>;
    type SerializeStructVariant = Compound<S::SerializeStructVariant>;

    fn is_human_readable(&self) -> bool {
        self.0.is_human_readable()
    }

    fn serialize_bool(self, v: bool) -> Result<S::Ok, S::Error> {
        self.0.serialize_bool(v)
    }

    fn serialize_i8(self, v: i8) -> Result<S::Ok, S::Error> {
        self.0.serialize_i8(v)
    }

    fn serialize_i16(self, v: i16) -> Result<S::Ok, S::Error> {
        self.0.serialize_i16(v)
    }

    fn serialize_i32(self, v: i32) -> Result<S::Ok, S::Error> {
        self.0.serialize_i32(v)
    }

    fn serialize_i64(self, v: i64) -> Result<S::Ok, S::Error> {
        self.0.serialize_i64(v)
    }

    fn serialize_i128(self, v: i128) -> Result<S::Ok, S::Error> {
        self.0.serialize_i128(v)
    }

    fn serialize_u8(self, v: u8) -> Result<S::Ok, S::Error> {
        self.0.serialize_u8(v)
    }

    fn serialize_u16(self, v: u16) -> Result<S::Ok, S::Error> {
        self.0.serialize_u16(v)
    }

    fn serialize_u32(self, v: u32) -> Result<S::Ok, S::Error> {
        self.0.serialize_u32(v)
    }

    fn serialize_u64(self, v: u64) -> Result<S::Ok, S::Error> {
        self.0.serialize_u64(v)
    }

    fn serialize_u128(self, v: u128) -> Result<S::Ok, S::Error> {
        self.0.serialize_u128(v)
    }

    fn serialize_f32(self, v: f32) -> Result<S::Ok, S::Error> {
        self.0.serialize_f32(v)
    }

    fn serialize_f64(self, v: f64) -> Result<S::Ok, S::Error> {
        self.0.serialize_f64(v)
    }

    fn serialize_char(self, v: char) -> Result<S::Ok, S::Error> {
        self.0.serialize_char(v)
    }

    fn serialize_str(self, v: &str) -> Result<S::Ok, S::Error> {
        self.0.serialize_str(v)
    }

    fn serialize_bytes(self, v: &[u8]) -> Result<S::Ok, S::Error> {
        self.0.serialize_bytes(v)
    }

    fn serialize_none(self) -> Result<S::Ok, S::Error> {
        self.0.serialize_none()
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<S::Ok, S::Error> {
        self.0.serialize_some(&Wrapped(value))
    }

    fn serialize_unit(self) -> Result<S::Ok, S::Error> {
        self.0.serialize_unit()
    }

    fn serialize_unit_struct(self, name: &'static str) -> Result<S::Ok, S::Error> {
        self.0.serialize_unit_struct(name)
    }

    fn serialize_unit_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
    ) -> Result<S::Ok, S::Error> {
        self.0.serialize_unit_variant(name, variant_index, variant)
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<S::Ok, S::Error> {
        self.0.serialize_newtype_struct(name, &Wrapped(value))
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<S::Ok, S::Error> {
        self.0
            .serialize_newtype_variant(name, variant_index, variant, &Wrapped(value))
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, S::Error> {
        self.0.serialize_seq(len).map(Compound)
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, S::Error> {
        self.0.serialize_tuple(len).map(Compound)
    }

    fn serialize_tuple_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, S::Error> {
        self.0.serialize_tuple_struct(name, len).map(Compound)
    }

    fn serialize_tuple_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, S::Error> {
        self.0
            .serialize_tuple_variant(name, variant_index, variant, len)
            .map(Compound)
    }

    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, S::Error> {
        self.0.serialize_map(len).map(Compound)
    }

    fn serialize_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, S::Error> {
        if name == RAW_VALUE_TOKEN {
            Ok(StructOrRawText::RawText {
                serializer: self.0,
                text: None,
            })
        } else {
            self.0
                .serialize_struct(name, len)
                .map(StructOrRawText::Struct)
        }
    }

    fn serialize_struct_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, S::Error> {
        self.0
            .serialize_struct_variant(name, variant_index, variant, len)
            .map(Compound)
    }
}

impl<C: ser::SerializeSeq> ser::SerializeSeq for Compound<C> {
    type Ok = C::Ok;
    type Error = C::Error;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), C::Error> {
        self.0.serialize_element(&Wrapped(value))
    }

    fn end(self) -> Result<C::Ok, C::Error> {
        self.0.end()
    }
}

impl<C: ser::SerializeTuple> ser::SerializeTuple for Compound<C> {
    type Ok = C::Ok;
    type Error = C::Error;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), C::Error> {
        self.0.serialize_element(&Wrapped(value))
    }

    fn end(self) -> Result<C::Ok, C::Error> {
        self.0.end()
    }
}

impl<C: ser::SerializeTupleStruct> ser::SerializeTupleStruct for Compound<C> {
    type Ok = C::Ok;
    type Error = C::Error;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), C::Error> {
        self.0.serialize_field(&Wrapped(value))
    }

    fn end(self) -> Result<C::Ok, C::Error> {
        self.0.end()
    }
}

impl<C: ser::SerializeTupleVariant> ser::SerializeTupleVariant for Compound<C> {
    type Ok = C::Ok;
    type Error = C::Error;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), C::Error> {
        self.0.serialize_field(&Wrapped(value))
    }

    fn end(self) -> Result<C::Ok, C::Error> {
        self.0.end()
    }
}

impl<C: ser::SerializeMap> ser::SerializeMap for Compound<C> {
    type Ok = C::Ok;
    type Error = C::Error;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), C::Error> {
        self.0.serialize_key(&Wrapped(key))
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), C::Error> {
        self.0.serialize_value(&Wrapped(value))
    }

    fn serialize_entry<K: Serialize + ?Sized, V: Serialize + ?Sized>(
        &mut self,
        key: &K,
        value: &V,
    ) -> Result<(), C::Error> {
        self.0.serialize_entry(&Wrapped(key), &Wrapped(value))
    }

    fn end(self) -> Result<C::Ok, C::Error> {
        self.0.end()
    }
}

impl<C: ser::SerializeStructVariant> ser::SerializeStructVariant for Compound<C> {
    type Ok = C::Ok;
    type Error = C::Error;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), C::Error> {
        self.0.serialize_field(key, &Wrapped(value))
    }

    fn skip_field(&mut self, key: &'static str) -> Result<(), C::Error> {
        self.0.skip_field(key)
    }

    fn end(self) -> Result<C::Ok, C::Error> {
        self.0.end()
    }
}

impl<S: Serializer> ser::SerializeStruct for StructOrRawText<S> {
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), S::Error> {
        match self {
            Self::Struct(fields) => fields.serialize_field(key, &Wrapped(value)),
            // `serde_json` gives the text of a raw JSON value as its only field, a string. The
            // string serializer of `Value` copies it, so the text stays exact.
            Self::RawText { text, .. } => {
                match value
                    .serialize(serde_json::value::Serializer)
                    .map_err(S::Error::custom)?
                {
                    serde_json::Value::String(raw) => {
                        *text = Some(raw);
                        Ok(())
                    }
                    _ => Err(S::Error::custom("a raw JSON value has no text")),
                }
            }
        }
    }

    fn skip_field(&mut self, key: &'static str) -> Result<(), S::Error> {
        match self {
            Self::Struct(fields) => fields.skip_field(key),
            Self::RawText { .. } => Ok(()),
        }
    }

    fn end(self) -> Result<S::Ok, S::Error> {
        match self {
            Self::Struct(fields) => fields.end(),
            Self::RawText {
                serializer,
                text: Some(text),
            } => serializer.serialize_str(&text),
            Self::RawText { text: None, .. } => {
                Err(S::Error::custom("a raw JSON value has no text"))
            }
        }
    }
}
