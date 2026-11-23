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

//! Reading of internally tagged enums without a buffer.
//!
//! A derived `Deserialize` of an internally tagged enum (`#[serde(tag = "...")]`) reads the whole
//! value into a buffer first, to find the tag. That buffer holds numbers as `u64`, `i64` or
//! `f64`. It rounds a larger number and refuses a number that no `f64` holds. So a field that
//! must keep its exact JSON text, such as the verified claims of a caller, cannot be inside such
//! an enum.
//!
//! The functions of this module read the value as its JSON text (`RawValue`), find the tag in
//! that text, and then read the variant from the same text. The variant reads the text directly,
//! so its own fields, and the fields of their fields, also see the exact text.
//!
//! The value must come from a `serde_json` reader or from a `serde_json::Value`. A container that
//! puts the value into a buffer first, such as another derived internally tagged or untagged
//! enum, cannot give the JSON text, and the read fails.

use serde::Deserialize;
use serde::de::{Deserializer, Error, IgnoredAny, MapAccess, Visitor};
use serde_json::value::RawValue;
use std::borrow::Cow;
use std::fmt::Formatter;

/// Reads the next value as its exact JSON text.
pub fn raw_value<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Box<RawValue>, D::Error> {
    Box::<RawValue>::deserialize(deserializer)
}

/// The string value of the member `tag` of the JSON object `raw`. The other members are skipped
/// without being read, so their numbers are never converted.
pub fn tag_of<E: Error>(raw: &RawValue, tag: &'static str) -> Result<String, E> {
    struct TagVisitor(&'static str);

    impl<'de> Visitor<'de> for TagVisitor {
        type Value = String;

        fn expecting(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            write!(f, "a JSON object with the member `{}`", self.0)
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<String, A::Error> {
            let mut found = None;
            while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                if key == self.0 && found.is_none() {
                    found = Some(map.next_value::<String>()?);
                } else {
                    map.next_value::<IgnoredAny>()?;
                }
            }
            found.ok_or_else(|| A::Error::missing_field(self.0))
        }
    }

    let mut deserializer = serde_json::Deserializer::from_str(raw.get());
    deserializer
        .deserialize_map(TagVisitor(tag))
        .map_err(E::custom)
}

/// Reads `T` from the JSON text `raw`.
pub fn parse<'a, T: Deserialize<'a>, E: Error>(raw: &'a RawValue) -> Result<T, E> {
    serde_json::from_str(raw.get()).map_err(E::custom)
}

/// Implements `Deserialize` for an internally tagged enum whose `Serialize` is derived with
/// `#[serde(tag = $tag)]`. Each variant holds one struct, and `$name` is the value of the tag
/// for that variant, as the derived `Serialize` writes it.
#[macro_export]
macro_rules! deserialize_internally_tagged {
    ($enum:ident, $tag:literal, { $($name:literal => $variant:ident),* $(,)? }) => {
        impl<'de> ::serde::Deserialize<'de> for $enum {
            fn deserialize<D: ::serde::Deserializer<'de>>(
                deserializer: D,
            ) -> Result<Self, D::Error> {
                let raw = $crate::base_model::raw_json::raw_value(deserializer)?;
                let tag: String = $crate::base_model::raw_json::tag_of::<D::Error>(&raw, $tag)?;
                match tag.as_str() {
                    $($name => $crate::base_model::raw_json::parse::<_, D::Error>(&raw).map(Self::$variant),)*
                    other => Err(<D::Error as ::serde::de::Error>::unknown_variant(
                        other,
                        &[$($name),*],
                    )),
                }
            }
        }
    };
}
