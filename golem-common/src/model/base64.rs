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

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bincode::de::{BorrowDecoder, Decoder};
use bincode::enc::Encoder;
use bincode::error::{DecodeError, EncodeError};
use bincode::{BorrowDecode, Decode, Encode};
#[cfg(feature = "poem")]
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
#[cfg(feature = "poem")]
use poem_openapi::types::ToJSON;
#[cfg(feature = "poem")]
use poem_openapi::types::{ParseError, ParseFromJSON, ParseFromParameter, ParseResult, Type};
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
#[cfg(feature = "poem")]
use serde_json::Value;
#[cfg(feature = "poem")]
use std::borrow::Cow;
use std::ops::{Deref, DerefMut};

/// Represents a binary data encoded with base64.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Default)]
pub struct Base64(pub Vec<u8>);

impl Deref for Base64 {
    type Target = Vec<u8>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Base64 {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[cfg(feature = "poem")]
impl Type for Base64 {
    const IS_REQUIRED: bool = true;

    type RawValueType = Self;

    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        "string_bytes".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        MetaSchemaRef::Inline(Box::new(MetaSchema::new_with_format("string", "bytes")))
    }

    fn as_raw_value(&self) -> Option<&Self::RawValueType> {
        Some(self)
    }

    fn raw_element_iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
        Box::new(self.as_raw_value().into_iter())
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(feature = "poem")]
impl ParseFromJSON for Base64 {
    fn parse_from_json(value: Option<Value>) -> ParseResult<Self> {
        let value = value.unwrap_or_default();
        if let Value::String(value) = value {
            Ok(Self(STANDARD.decode(value)?))
        } else {
            Err(ParseError::expected_type(value))
        }
    }
}

#[cfg(feature = "poem")]
impl ParseFromParameter for Base64 {
    fn parse_from_parameter(value: &str) -> ParseResult<Self> {
        Ok(Self(STANDARD.decode(value)?))
    }
}

#[cfg(feature = "poem")]
impl ToJSON for Base64 {
    fn to_json(&self) -> Option<Value> {
        let b64 = STANDARD.encode(&self.0);
        Some(Value::String(b64))
    }
}

impl Serialize for Base64 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let b64 = STANDARD.encode(&self.0);
        serializer.serialize_str(&b64)
    }
}

impl<'de> Deserialize<'de> for Base64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let b64: String = String::deserialize(deserializer)?;
        Ok(Base64(
            STANDARD
                .decode(b64)
                .map_err(|err| Error::custom(err.to_string()))?,
        ))
    }
}

impl Encode for Base64 {
    fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        self.0.encode(encoder)
    }
}

impl Decode for Base64 {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let vec: Vec<u8> = Vec::decode(decoder)?;
        Ok(Base64(vec))
    }
}

impl<'de> BorrowDecode<'de> for Base64 {
    fn borrow_decode<D: BorrowDecoder<'de>>(decoder: &mut D) -> Result<Self, DecodeError> {
        let vec: Vec<u8> = Vec::borrow_decode(decoder)?;
        Ok(Base64(vec))
    }
}
