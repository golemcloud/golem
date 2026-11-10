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

use crate::schema::conversion::{FromSchema, FromSchemaError};
use crate::schema::schema_value::SchemaValue;
use serde::{Deserialize, Serialize};

/// Environment-local identity shared by agent types and tools. Zero is reserved.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    golem_schema_derive::IntoSchema,
)]
#[cfg_attr(feature = "full", derive(poem_openapi::NewType))]
#[cfg_attr(
    feature = "full",
    oai(
        example,
        from_json = false,
        from_parameter = false,
        from_multipart = false
    )
)]
#[serde(try_from = "u32", into = "u32")]
#[schema(transparent)]
pub struct TypeId(u32);

impl TypeId {
    pub fn get(self) -> u32 {
        self.0
    }
}

#[cfg(feature = "full")]
impl poem_openapi::types::Example for TypeId {
    fn example() -> Self {
        Self(1)
    }
}

impl TryFrom<u32> for TypeId {
    type Error = String;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        if value == 0 {
            Err("Type ID zero is reserved".to_string())
        } else {
            Ok(Self(value))
        }
    }
}

impl TryFrom<i64> for TypeId {
    type Error = String;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        u32::try_from(value)
            .map_err(|_| format!("Type ID {value} is outside the 32-bit range"))?
            .try_into()
    }
}

impl From<TypeId> for u32 {
    fn from(value: TypeId) -> Self {
        value.0
    }
}

impl From<TypeId> for i64 {
    fn from(value: TypeId) -> Self {
        i64::from(value.0)
    }
}

impl FromSchema for TypeId {
    fn from_value(value: &SchemaValue) -> Result<Self, FromSchemaError> {
        Self::try_from(u32::from_value(value)?).map_err(FromSchemaError::custom)
    }
}

#[cfg(feature = "full")]
impl poem_openapi::types::ParseFromJSON for TypeId {
    fn parse_from_json(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
        let value = <u32 as poem_openapi::types::ParseFromJSON>::parse_from_json(value)
            .map_err(poem_openapi::types::ParseError::propagate)?;
        Self::try_from(value).map_err(poem_openapi::types::ParseError::custom)
    }
}

#[cfg(feature = "full")]
impl poem_openapi::types::ParseFromParameter for TypeId {
    fn parse_from_parameter(value: &str) -> poem_openapi::types::ParseResult<Self> {
        let value = <u32 as poem_openapi::types::ParseFromParameter>::parse_from_parameter(value)
            .map_err(poem_openapi::types::ParseError::propagate)?;
        Self::try_from(value).map_err(poem_openapi::types::ParseError::custom)
    }
}

#[cfg(feature = "full")]
impl poem_openapi::types::ParseFromMultipartField for TypeId {
    async fn parse_from_multipart(
        field: Option<poem::web::Field>,
    ) -> poem_openapi::types::ParseResult<Self> {
        let value =
            <u32 as poem_openapi::types::ParseFromMultipartField>::parse_from_multipart(field)
                .await
                .map_err(poem_openapi::types::ParseError::propagate)?;
        Self::try_from(value).map_err(poem_openapi::types::ParseError::custom)
    }
}

#[cfg(feature = "full")]
impl desert_rust::BinarySerializer for TypeId {
    fn serialize<Output: desert_rust::BinaryOutput>(
        &self,
        context: &mut desert_rust::SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        desert_rust::BinarySerializer::serialize(&self.0, context)
    }
}

#[cfg(feature = "full")]
impl desert_rust::BinaryDeserializer for TypeId {
    fn deserialize<'a, 'b>(
        context: &'a mut desert_rust::DeserializationContext<'b>,
    ) -> desert_rust::Result<Self> {
        let value: u32 = desert_rust::BinaryDeserializer::deserialize(context)?;
        Self::try_from(value).map_err(desert_rust::Error::DeserializationFailure)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
#[cfg_attr(feature = "full", oai(rename_all = "kebab-case"))]
#[serde(rename_all = "kebab-case")]
pub enum TypeIdKind {
    AgentType,
    Tool,
}

impl TypeIdKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentType => "agent-type",
            Self::Tool => "tool",
        }
    }
}

impl std::str::FromStr for TypeIdKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "agent-type" => Ok(Self::AgentType),
            "tool" => Ok(Self::Tool),
            _ => Err(format!("Unknown type ID kind: {value}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::conversion::IntoSchema;
    use test_r::test;

    #[test]
    fn type_id_checked_conversions() {
        assert!(TypeId::try_from(0u32).is_err());
        for value in [1, u32::MAX] {
            let id = TypeId::try_from(value).unwrap();
            assert_eq!(id.get(), value);
            assert_eq!(u32::from(id), value);
            assert_eq!(i64::from(id), i64::from(value));
            assert_eq!(TypeId::try_from(i64::from(value)).unwrap(), id);
        }
        for value in [i64::MIN, -1, 0, i64::from(u32::MAX) + 1, i64::MAX] {
            assert!(TypeId::try_from(value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn type_id_serde_validation() {
        for value in ["0", "-1", "4294967296", "null", "\"1\""] {
            assert!(serde_json::from_str::<TypeId>(value).is_err());
        }
        for value in [1, u32::MAX] {
            let id = TypeId::try_from(value).unwrap();
            let json = serde_json::to_string(&id).unwrap();
            assert_eq!(json, value.to_string());
            assert_eq!(serde_json::from_str::<TypeId>(&json).unwrap(), id);
        }
    }

    #[test]
    fn type_id_schema_validation() {
        #[derive(
            Debug, PartialEq, golem_schema_derive::IntoSchema, golem_schema_derive::FromSchema,
        )]
        struct Registration {
            type_id: TypeId,
        }

        assert!(TypeId::from_value(&SchemaValue::U32(0)).is_err());
        assert!(TypeId::from_value(&SchemaValue::String("1".to_string())).is_err());
        for value in [1, u32::MAX] {
            let id = TypeId::try_from(value).unwrap();
            assert_eq!(id.to_value(), SchemaValue::U32(value));
            assert_eq!(TypeId::from_value(&id.to_value()).unwrap(), id);
            let registration = Registration { type_id: id };
            assert_eq!(
                Registration::from_value(&registration.to_value()).unwrap(),
                registration
            );
        }
        crate::schema::conversion::try_into_schema_graph::<TypeId>().unwrap();
        crate::schema::conversion::try_into_schema_graph::<Registration>().unwrap();
    }

    #[cfg(feature = "full")]
    #[test]
    fn type_id_binary_validation() {
        let zero = desert_rust::serialize_to_byte_vec(&0u32).unwrap();
        assert!(desert_rust::deserialize::<TypeId>(&zero).is_err());
        for value in [1, u32::MAX] {
            let id = TypeId::try_from(value).unwrap();
            let bytes = desert_rust::serialize_to_byte_vec(&id).unwrap();
            assert_eq!(bytes, desert_rust::serialize_to_byte_vec(&value).unwrap());
            assert_eq!(desert_rust::deserialize::<TypeId>(&bytes).unwrap(), id);
        }
    }

    #[cfg(feature = "full")]
    #[test]
    fn type_id_poem_validation() {
        use poem_openapi::types::{ParseFromJSON, ParseFromParameter};

        for value in [0, -1, i64::from(u32::MAX) + 1] {
            assert!(TypeId::parse_from_json(Some(serde_json::json!(value))).is_err());
            assert!(TypeId::parse_from_parameter(&value.to_string()).is_err());
            assert!(TypeId::parse_from_parameters([value.to_string()]).is_err());
        }
        for value in [1, u32::MAX] {
            let id = TypeId::try_from(value).unwrap();
            assert_eq!(
                TypeId::parse_from_json(Some(serde_json::json!(value))).unwrap(),
                id
            );
            assert_eq!(
                TypeId::parse_from_parameter(&value.to_string()).unwrap(),
                id
            );
        }
    }

    #[cfg(feature = "full")]
    #[test]
    async fn type_id_poem_multipart_validation() {
        use poem::FromRequest;
        use poem_openapi::types::ParseFromMultipartField;

        assert!(TypeId::parse_from_multipart(None).await.is_err());
        for value in [0, -1, 1, i64::from(u32::MAX), i64::from(u32::MAX) + 1] {
            let body = format!(
                "--BOUNDARY\r\nContent-Disposition: form-data; name=\"type_id\"\r\n\r\n{value}\r\n--BOUNDARY--\r\n"
            );
            let request = poem::Request::builder()
                .header("content-type", "multipart/form-data; boundary=BOUNDARY")
                .body(body);
            let (request, mut body) = request.split();
            let mut multipart = poem::web::Multipart::from_request(&request, &mut body)
                .await
                .unwrap();
            let parsed = TypeId::parse_from_multipart(multipart.next_field().await.unwrap()).await;
            match TypeId::try_from(value) {
                Ok(expected) => assert_eq!(parsed.unwrap(), expected),
                Err(_) => assert!(parsed.is_err(), "accepted {value}"),
            }
        }
    }
}
