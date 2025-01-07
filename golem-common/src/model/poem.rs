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

use crate::model::{
    AccountId, ComponentFilePath, ComponentFilePathWithPermissionsList, IdempotencyKey, Timestamp,
};
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{ParseFromJSON, ParseFromParameter, ParseResult, ToJSON};
use serde_json::Value;
use std::borrow::Cow;

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

impl poem_openapi::types::Type for AccountId {
    const IS_REQUIRED: bool = true;
    type RawValueType = Self;
    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        Cow::from("string(account_id)")
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

impl ParseFromParameter for AccountId {
    fn parse_from_parameter(value: &str) -> ParseResult<Self> {
        Ok(Self {
            value: value.to_string(),
        })
    }
}

impl ParseFromJSON for AccountId {
    fn parse_from_json(value: Option<Value>) -> ParseResult<Self> {
        match value {
            Some(Value::String(s)) => Ok(Self { value: s }),
            _ => Err(poem_openapi::types::ParseError::<AccountId>::custom(
                "Unexpected representation of AccountId".to_string(),
            )),
        }
    }
}

impl ToJSON for AccountId {
    fn to_json(&self) -> Option<Value> {
        Some(Value::String(self.value.clone()))
    }
}

impl poem_openapi::types::Type for ComponentFilePath {
    const IS_REQUIRED: bool = true;

    type RawValueType = Self;

    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        "string".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        MetaSchemaRef::Inline(Box::new(MetaSchema {
            description: Some("Path inside a component filesystem. Must be absolute."),
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

impl poem_openapi::types::ToJSON for ComponentFilePath {
    fn to_json(&self) -> Option<serde_json::Value> {
        Some(serde_json::Value::String(self.to_string()))
    }
}

impl poem_openapi::types::ParseFromJSON for ComponentFilePath {
    fn parse_from_json(
        value: Option<serde_json::Value>,
    ) -> Result<Self, poem_openapi::types::ParseError<Self>> {
        match value {
            None => Err(poem_openapi::types::ParseError::custom(
                "Missing value for ComponentFilePath",
            )),
            Some(value) => {
                serde_json::from_value(value).map_err(poem_openapi::types::ParseError::custom)
            }
        }
    }
}

impl poem_openapi::types::ParseFromMultipartField for ComponentFilePathWithPermissionsList {
    async fn parse_from_multipart(field: Option<poem::web::Field>) -> ParseResult<Self> {
        String::parse_from_multipart(field)
            .await
            .map_err(|err| err.propagate::<ComponentFilePathWithPermissionsList>())
            .and_then(|s| serde_json::from_str(&s).map_err(poem_openapi::types::ParseError::custom))
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::model::{
        ComponentFilePath, ComponentFilePermissions, Empty, IdempotencyKey, InitialComponentFile,
        InitialComponentFileKey, WorkerStatus,
    };
    use poem_openapi::types::ToJSON;

    #[test]
    fn worker_status_serialization_poem_serde_equivalence() {
        let status = WorkerStatus::Retrying;
        let serialized = status.to_json_string();
        let deserialized: WorkerStatus = serde_json::from_str(&serialized).unwrap();
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
        let file = InitialComponentFile {
            key: InitialComponentFileKey("key".to_string()),
            path: ComponentFilePath::from_rel_str("hello").unwrap(),
            permissions: ComponentFilePermissions::ReadWrite,
        };
        let serialized = file.to_json_string();
        let deserialized: InitialComponentFile = serde_json::from_str(&serialized).unwrap();
        assert_eq!(file, deserialized);
    }
}
