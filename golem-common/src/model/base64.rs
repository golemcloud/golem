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

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::ToJSON;
use poem_openapi::types::{ParseError, ParseFromJSON, ParseFromParameter, ParseResult, Type};
use serde_json::Value;
use std::borrow::Cow;

pub use crate::base_model::base64::*;

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

impl ParseFromParameter for Base64 {
    fn parse_from_parameter(value: &str) -> ParseResult<Self> {
        Ok(Self(STANDARD.decode(value)?))
    }
}

impl ToJSON for Base64 {
    fn to_json(&self) -> Option<Value> {
        let b64 = STANDARD.encode(&self.0);
        Some(Value::String(b64))
    }
}
