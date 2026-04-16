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

use crate::base_model::agent::AgentFileContentHash;
use crate::base_model::json::NormalizedJsonValue;
use crate::model::component::{AgentFilePath, ArchiveFilePath, CanonicalFilePath};
use crate::model::{IdempotencyKey, Timestamp};
use poem_openapi::ApiResponse;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{ParseFromJSON, ParseFromParameter, ParseResult, ToJSON};
use serde_json::Value;
use std::borrow::Cow;

#[derive(Debug, Clone, ApiResponse)]
pub enum NoContentResponse {
    #[oai(status = 204)]
    NoContent,
}

impl poem_openapi::types::Type for Timestamp {
    const IS_REQUIRED: bool = true;
    type RawValueType = Self;
    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        Cow::from("string(timestamp)")
    }

    fn schema_ref() -> MetaSchemaRef {
        MetaSchemaRef::Inline(Box::new(MetaSchema::new_with_format("string", "date-time")))
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

impl ToJSON for Timestamp {
    fn to_json(&self) -> Option<Value> {
        Some(Value::String(self.0.to_string()))
    }
}

impl ParseFromParameter for Timestamp {
    fn parse_from_parameter(value: &str) -> ParseResult<Self> {
        value.parse().map_err(|_| {
            poem_openapi::types::ParseError::<Timestamp>::custom(
                "Unexpected representation of timestamp".to_string(),
            )
        })
    }
}

impl ParseFromJSON for Timestamp {
    fn parse_from_json(value: Option<Value>) -> ParseResult<Self> {
        match value {
            Some(Value::String(s)) => Timestamp::parse_from_parameter(&s),
            _ => Err(poem_openapi::types::ParseError::<Timestamp>::custom(
                "Unexpected representation of timestamp".to_string(),
            )),
        }
    }
}

impl poem_openapi::types::Type for IdempotencyKey {
    const IS_REQUIRED: bool = true;
    type RawValueType = Self;
    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        Cow::from(format!("string({})", stringify!(InvocationKey)))
    }

    fn schema_ref() -> MetaSchemaRef {
        MetaSchemaRef::Inline(Box::new(MetaSchema::new("string")))
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

impl ParseFromParameter for IdempotencyKey {
    fn parse_from_parameter(value: &str) -> ParseResult<Self> {
        Ok(Self {
            value: value.to_string(),
        })
    }
}

impl ParseFromJSON for IdempotencyKey {
    fn parse_from_json(value: Option<Value>) -> ParseResult<Self> {
        match value {
            Some(Value::String(s)) => Ok(Self { value: s }),
            _ => Err(poem_openapi::types::ParseError::<IdempotencyKey>::custom(
                format!("Unexpected representation of {}", stringify!(InvocationKey)),
            )),
        }
    }
}

impl ToJSON for IdempotencyKey {
    fn to_json(&self) -> Option<Value> {
        Some(Value::String(self.value.clone()))
    }
}

impl poem_openapi::types::Type for CanonicalFilePath {
    const IS_REQUIRED: bool = true;

    type RawValueType = Self;

    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        "string".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        MetaSchemaRef::Inline(Box::new(MetaSchema {
            description: Some("A canonical, absolute, normalized file path. Must start with '/'."),
            ..MetaSchema::new("string")
        }))
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

impl poem_openapi::types::ToJSON for CanonicalFilePath {
    fn to_json(&self) -> Option<serde_json::Value> {
        Some(serde_json::Value::String(self.to_string()))
    }
}

impl poem_openapi::types::ParseFromJSON for CanonicalFilePath {
    fn parse_from_json(
        value: Option<serde_json::Value>,
    ) -> Result<Self, poem_openapi::types::ParseError<Self>> {
        match value {
            None => Err(poem_openapi::types::ParseError::custom(
                "Missing value for CanonicalFilePath",
            )),
            Some(value) => {
                serde_json::from_value(value).map_err(poem_openapi::types::ParseError::custom)
            }
        }
    }
}

impl poem_openapi::types::Type for AgentFileContentHash {
    const IS_REQUIRED: bool = true;
    type RawValueType = Self;
    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        crate::model::diff::Hash::name()
    }

    fn schema_ref() -> MetaSchemaRef {
        crate::model::diff::Hash::schema_ref()
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

impl poem_openapi::types::ToJSON for AgentFileContentHash {
    fn to_json(&self) -> Option<Value> {
        self.0.to_json()
    }
}

impl poem_openapi::types::ParseFromJSON for AgentFileContentHash {
    fn parse_from_json(
        value: Option<Value>,
    ) -> Result<Self, poem_openapi::types::ParseError<Self>> {
        crate::model::diff::Hash::parse_from_json(value)
            .map(AgentFileContentHash)
            .map_err(poem_openapi::types::ParseError::propagate)
    }
}

impl poem_openapi::types::Type for NormalizedJsonValue {
    const IS_REQUIRED: bool = true;
    type RawValueType = Self;
    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        "NormalizedJsonValue".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        MetaSchemaRef::Reference(Self::name().into_owned())
    }

    fn register(registry: &mut poem_openapi::registry::Registry) {
        registry.create_schema::<Self, _>(Self::name().into_owned(), |_| {
            // Any valid JSON value — no structural constraints
            poem_openapi::registry::MetaSchema::new("object")
        });
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

impl poem_openapi::types::ToJSON for NormalizedJsonValue {
    fn to_json(&self) -> Option<Value> {
        Some(self.0.clone())
    }
}

impl poem_openapi::types::ParseFromJSON for NormalizedJsonValue {
    fn parse_from_json(value: Option<Value>) -> poem_openapi::types::ParseResult<Self> {
        Ok(NormalizedJsonValue::from(value.unwrap_or(Value::Null)))
    }
}

impl poem_openapi::types::Type for ArchiveFilePath {
    const IS_REQUIRED: bool = true;
    type RawValueType = Self;
    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        "string".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        MetaSchemaRef::Inline(Box::new(MetaSchema {
            description: Some("Path of a file inside an uploaded archive."),
            ..MetaSchema::new("string")
        }))
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

impl poem_openapi::types::ToJSON for ArchiveFilePath {
    fn to_json(&self) -> Option<serde_json::Value> {
        Some(serde_json::Value::String(self.to_abs_string()))
    }
}

impl poem_openapi::types::ParseFromJSON for ArchiveFilePath {
    fn parse_from_json(
        value: Option<serde_json::Value>,
    ) -> Result<Self, poem_openapi::types::ParseError<Self>> {
        match value {
            None => Err(poem_openapi::types::ParseError::custom(
                "Missing value for ArchiveFilePath",
            )),
            Some(v) => serde_json::from_value(v).map_err(poem_openapi::types::ParseError::custom),
        }
    }
}

impl poem_openapi::types::Type for AgentFilePath {
    const IS_REQUIRED: bool = true;
    type RawValueType = Self;
    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        "string".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        MetaSchemaRef::Inline(Box::new(MetaSchema {
            description: Some("Absolute path in an agent's filesystem."),
            ..MetaSchema::new("string")
        }))
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

impl poem_openapi::types::ToJSON for AgentFilePath {
    fn to_json(&self) -> Option<serde_json::Value> {
        Some(serde_json::Value::String(self.to_abs_string()))
    }
}

impl poem_openapi::types::ParseFromJSON for AgentFilePath {
    fn parse_from_json(
        value: Option<serde_json::Value>,
    ) -> Result<Self, poem_openapi::types::ParseError<Self>> {
        match value {
            None => Err(poem_openapi::types::ParseError::custom(
                "Missing value for AgentFilePath",
            )),
            Some(v) => serde_json::from_value(v).map_err(poem_openapi::types::ParseError::custom),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::base_model::agent::AgentFileContentHash;
    use crate::model::component::{AgentFilePath, InitialAgentFile};
    use crate::model::{AgentFilePermissions, AgentStatus, Empty, IdempotencyKey};
    use poem_openapi::types::ToJSON;
    use test_r::test;

    #[test]
    fn worker_status_serialization_poem_serde_equivalence() {
        let status = AgentStatus::Retrying;
        let serialized = status.to_json_string();
        let deserialized: AgentStatus = serde_json::from_str(&serialized).unwrap();
        assert_eq!(status, deserialized);
    }

    #[test]
    fn idempotency_key_serialization_poem_serde_equivalence() {
        let key = IdempotencyKey::fresh();
        let serialized = key.to_json_string();
        let deserialized: IdempotencyKey = serde_json::from_str(&serialized).unwrap();
        assert_eq!(key, deserialized);
    }

    #[test]
    fn empty_poem_serde_equivalence() {
        let serialized = Empty {}.to_json_string();
        let deserialized: Empty = serde_json::from_str(&serialized).unwrap();
        assert_eq!(Empty {}, deserialized);
    }

    #[test]
    fn initial_component_file_serde_equivalence() {
        let file = InitialAgentFile {
            content_hash: AgentFileContentHash(
                blake3::Hash::from_bytes([
                    143, 27, 202, 64, 119, 5, 88, 233, 14, 191, 62, 209, 76, 8, 154, 240, 37, 121,
                    196, 3, 255, 98, 41, 172, 67, 10, 184, 213, 52, 139, 201, 16,
                ])
                .into(),
            ),
            path: AgentFilePath::from_rel_str("hello").unwrap(),
            permissions: AgentFilePermissions::ReadWrite,
            size: 1234,
        };
        let serialized = file.to_json_string();
        let deserialized: InitialAgentFile = serde_json::from_str(&serialized).unwrap();
        assert_eq!(file, deserialized);
    }
}
