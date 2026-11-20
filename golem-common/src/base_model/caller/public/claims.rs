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

use super::super::Claims;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;

/// The verified claims of a user, as the public form shows them: one JSON object, kept as the
/// exact text that Golem received.
///
/// The text is never parsed into a `serde_json::Value`. A `Value` rounds numbers, fails on
/// numbers that no `f64` holds, and stops at a nesting depth of 128. Write this type with
/// `serde_json::to_string` or `to_writer`. `serde_json::to_value` parses the text again and so
/// loses these exact bytes, or fails.
///
/// A `serde_json` reader keeps the exact text. A reader must give the JSON text: a `serde_json`
/// reader or a `serde_json::Value`. A container that buffers the value first, such as a derived
/// internally tagged enum, cannot, and the read fails. The tagged enums of the public oplog and
/// of the public caller read their values without a buffer for this reason (see `raw_json`).
#[derive(Debug, Clone)]
pub struct PublicClaims(Box<RawValue>);

impl PublicClaims {
    /// Reads claims from JSON text. The text must be one JSON object.
    pub fn from_json(json: String) -> Result<Self, String> {
        let raw = RawValue::from_string(json).map_err(|error| error.to_string())?;
        Self::from_raw(raw)
    }

    fn from_raw(raw: Box<RawValue>) -> Result<Self, String> {
        if raw.get().starts_with('{') {
            Ok(Self(raw))
        } else {
            Err("the claim set must be a JSON object".to_string())
        }
    }

    /// Writes a JSON value as claims. For a `Value` from a parser, the number limits of `Value`
    /// have already applied.
    pub fn from_value(value: &serde_json::Value) -> Result<Self, String> {
        Self::from_json(value.to_string())
    }

    /// The claims as a `serde_json::Value`. This fails for claims with a number that no `f64`
    /// holds. Read `as_json` to get the exact text.
    pub fn to_value(&self) -> serde_json::Result<serde_json::Value> {
        serde_json::from_str(self.as_json())
    }

    /// The claims as JSON text, exactly as Golem received them.
    pub fn as_json(&self) -> &str {
        self.0.get()
    }
}

impl From<&Claims> for PublicClaims {
    fn from(claims: &Claims) -> Self {
        // `Claims` holds one valid JSON object, so the text needs no new check.
        Self(
            RawValue::from_string(claims.as_json().to_string())
                .expect("claims hold valid JSON text"),
        )
    }
}

impl PartialEq for PublicClaims {
    fn eq(&self, other: &Self) -> bool {
        self.as_json() == other.as_json()
    }
}

impl Eq for PublicClaims {}

impl std::fmt::Display for PublicClaims {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_json())
    }
}

impl Serialize for PublicClaims {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PublicClaims {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = crate::base_model::raw_json::raw_value(deserializer)?;
        Self::from_raw(raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(feature = "full")]
mod openapi {
    use super::PublicClaims;
    use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
    use poem_openapi::types::{ParseError, ParseFromJSON, ParseResult, ToJSON, Type};
    use std::borrow::Cow;

    /// The OpenAPI schema is the same as the one of `serde_json::Value`: any JSON.
    impl Type for PublicClaims {
        const IS_REQUIRED: bool = true;

        type RawValueType = Self;

        type RawElementValueType = Self;

        fn name() -> Cow<'static, str> {
            "any".into()
        }

        fn schema_ref() -> MetaSchemaRef {
            MetaSchemaRef::Inline(Box::new(MetaSchema::ANY))
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

    impl ParseFromJSON for PublicClaims {
        fn parse_from_json(value: Option<serde_json::Value>) -> ParseResult<Self> {
            let value = value.ok_or_else(|| ParseError::custom("the claims are missing"))?;
            Self::from_json(value.to_string()).map_err(ParseError::custom)
        }
    }

    /// This form is lossy: a `serde_json::Value` cannot hold every claim set. The oplog
    /// endpoints do not use it. They write their response with serde, which keeps the text.
    impl ToJSON for PublicClaims {
        fn to_json(&self) -> Option<serde_json::Value> {
            Some(
                serde_json::from_str(self.as_json())
                    .unwrap_or_else(|_| serde_json::Value::String(self.as_json().to_string())),
            )
        }
    }
}
