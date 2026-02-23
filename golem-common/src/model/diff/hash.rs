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

pub use crate::base_model::diff::hash::*;
use crate::model::diff::Diffable;
use crate::model::diff::ser::{SerializeMode, ToSerializableWithMode, to_json_with_mode};
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};
use std::sync::OnceLock;

pub trait Hashable {
    fn hash(&self) -> Hash;
}

#[derive(Debug, Clone)]
pub enum HashOfKind<V> {
    Precalculated(Hash),
    FromValue { value: V, lazy_hash: OnceLock<Hash> },
}

#[derive(Debug, Clone)]
pub struct HashOf<V>(HashOfKind<V>);

impl<V> HashOf<V> {
    pub fn from_hash(hash: Hash) -> Self {
        Self(HashOfKind::Precalculated(hash))
    }

    pub fn from_blake3_hash(hash: blake3::Hash) -> Self {
        Self(HashOfKind::Precalculated(hash.into()))
    }

    pub fn form_value(value: V) -> Self {
        Self(HashOfKind::FromValue {
            value,
            lazy_hash: OnceLock::new(),
        })
    }

    pub fn as_value(&self) -> Option<&V> {
        match &self.0 {
            HashOfKind::Precalculated(_) => None,
            HashOfKind::FromValue { value, .. } => Some(value),
        }
    }
}

impl<V: Hashable> Hashable for HashOf<V> {
    fn hash(&self) -> Hash {
        match &self.0 {
            HashOfKind::Precalculated(hash) => *hash,
            HashOfKind::FromValue { value, lazy_hash } => *lazy_hash.get_or_init(|| value.hash()),
        }
    }
}

impl<V: Hashable> PartialEq for HashOf<V> {
    fn eq(&self, other: &Self) -> bool {
        self.hash() == other.hash()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DiffForHashOf<V: Diffable> {
    HashDiff { new_hash: Hash, current_hash: Hash },
    ValueDiff { diff: V::DiffResult },
}

impl<V: Hashable + Diffable> Diffable for HashOf<V> {
    type DiffResult = DiffForHashOf<V>;

    fn diff(new: &Self, current: &Self) -> Option<Self::DiffResult> {
        if new == current {
            return None;
        }

        let new_hash = new.hash();
        let current_hash = current.hash();

        let diff = match (new.as_value(), current.as_value()) {
            (Some(new), Some(current)) => new.diff_with_current(current),
            _ => None,
        };

        match diff {
            Some(diff) => Some(DiffForHashOf::ValueDiff { diff }),
            None => Some(DiffForHashOf::HashDiff {
                new_hash,
                current_hash,
            }),
        }
    }
}

impl<V: Diffable> Serialize for DiffForHashOf<V> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            DiffForHashOf::HashDiff {
                new_hash,
                current_hash,
            } => {
                let mut s = serializer.serialize_struct("DiffForHashOfByHashes", 2)?;
                s.serialize_field("newHash", new_hash)?;
                s.serialize_field("currentHash", current_hash)?;
                s.end()
            }
            DiffForHashOf::ValueDiff { diff } => diff.serialize(serializer),
        }
    }
}

impl<V: Hashable> From<V> for HashOf<V> {
    fn from(value: V) -> Self {
        Self::form_value(value)
    }
}

impl<V: Hashable> From<Hash> for HashOf<V> {
    fn from(value: Hash) -> Self {
        Self::from_hash(value)
    }
}

impl<V: Hashable> From<blake3::Hash> for HashOf<V> {
    fn from(value: blake3::Hash) -> Self {
        Self::from_hash(value.into())
    }
}

impl<V: Hashable + Serialize> ToSerializableWithMode for HashOf<V> {
    fn to_serializable(&self, mode: SerializeMode) -> serde_json::Value {
        match mode {
            SerializeMode::HashOnly => {
                serde_json::Value::String(self.hash().0.to_hex().to_string())
            }
            SerializeMode::ValueIfAvailable => match &self.0 {
                HashOfKind::Precalculated(hash) => {
                    serde_json::Value::String(hash.0.to_hex().to_string())
                }
                HashOfKind::FromValue {
                    value,
                    lazy_hash: _,
                } => serde_json::to_value(value)
                    .expect("failed to convert value to JSON for hashing"),
            },
        }
    }
}

pub fn hash_from_serialized_value<T: Serialize>(value: &T) -> Hash {
    blake3::hash(
        to_json_with_mode(value, SerializeMode::HashOnly)
            .expect("failed to serialize as JSON for hashing")
            .as_bytes(),
    )
    .into()
}

mod poem {
    use super::Hash;
    use http::HeaderValue;
    use poem::web::Field;
    use poem_openapi::types::{
        ParseError, ParseFromJSON, ParseFromMultipartField, ParseFromParameter, ParseResult,
        ToHeader, ToJSON,
    };
    use serde_json::Value;

    impl poem_openapi::types::Type for Hash {
        const IS_REQUIRED: bool = true;
        type RawValueType = Self;
        type RawElementValueType = Self;

        fn name() -> std::borrow::Cow<'static, str> {
            std::borrow::Cow::from("string_hash")
        }

        fn schema_ref() -> poem_openapi::registry::MetaSchemaRef {
            poem_openapi::registry::MetaSchemaRef::Inline(Box::new(
                poem_openapi::registry::MetaSchema::new_with_format("string", "hash"),
            ))
        }

        fn as_raw_value(&self) -> Option<&Self::RawValueType> {
            Some(self)
        }

        fn raw_element_iter<'a>(
            &'a self,
        ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
            Box::new(self.as_raw_value().into_iter())
        }
    }

    impl ParseFromJSON for Hash {
        fn parse_from_json(value: Option<Value>) -> ParseResult<Self> {
            let value = value.unwrap_or_default();
            if let Value::String(value) = value {
                Ok(Hash(blake3::Hash::from_hex(value)?))
            } else {
                Err(ParseError::expected_type(value))
            }
        }
    }

    impl ParseFromParameter for Hash {
        fn parse_from_parameter(value: &str) -> ParseResult<Self> {
            Ok(Hash(blake3::Hash::from_hex(value)?))
        }
    }

    impl ParseFromMultipartField for Hash {
        async fn parse_from_multipart(field: Option<Field>) -> ParseResult<Self> {
            match field {
                Some(field) => {
                    let value = field.text().await?;
                    Ok(Hash(blake3::Hash::from_hex(value)?))
                }
                None => Err(ParseError::expected_input()),
            }
        }
    }

    impl ToJSON for Hash {
        fn to_json(&self) -> Option<Value> {
            Some(Value::String(self.to_string()))
        }
    }

    impl ToHeader for Hash {
        fn to_header(&self) -> Option<HeaderValue> {
            HeaderValue::from_str(&self.to_string()).ok()
        }
    }
}
