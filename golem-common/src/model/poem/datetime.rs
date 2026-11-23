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

use std::borrow::Cow;

use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{
    ParseError, ParseFromJSON, ParseFromParameter, ParseResult, ToJSON, Type,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The OpenAPI representation of a Jiff timestamp. Domain fields remain Jiff
/// values; this type is used only by the schema and JSON boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct OpenApiTimestamp(pub jiff::Timestamp);

impl OpenApiTimestamp {
    fn parse(value: &str) -> Result<Self, crate::schema::canonical::error::ParseError> {
        let timestamp = crate::schema::canonical::datetime::from_text(value)?;
        // UTC normalization must not move the instant outside the API's year range.
        crate::schema::canonical::datetime::to_text(&timestamp)?;
        Ok(Self(timestamp))
    }
}

impl<'de> Deserialize<'de> for OpenApiTimestamp {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

impl Type for OpenApiTimestamp {
    const IS_REQUIRED: bool = true;
    type RawValueType = Self;
    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        "string_date-time".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        MetaSchemaRef::Inline(Box::new(MetaSchema::new_with_format("string", "date-time")))
    }

    fn as_raw_value(&self) -> Option<&Self> {
        Some(self)
    }

    fn raw_element_iter<'a>(&'a self) -> Box<dyn Iterator<Item = &'a Self> + 'a> {
        Box::new(std::iter::once(self))
    }
}

impl ParseFromJSON for OpenApiTimestamp {
    fn parse_from_json(value: Option<Value>) -> ParseResult<Self> {
        serde_json::from_value(value.unwrap_or(Value::Null)).map_err(ParseError::custom)
    }
}

impl ParseFromParameter for OpenApiTimestamp {
    fn parse_from_parameter(value: &str) -> ParseResult<Self> {
        Self::parse(value).map_err(ParseError::custom)
    }
}

impl ToJSON for OpenApiTimestamp {
    fn to_json(&self) -> Option<Value> {
        Some(Value::String(self.0.to_string()))
    }
}

/// Supplies the OpenAPI implementation through a private boundary object. The
/// boundary fields can use OpenAPI adapters without changing the domain types.
#[macro_export]
macro_rules! datetime_object {
    ($original:path, $name:ident, [$($example:item)*], $($boundary:item)*) => {
        #[cfg(feature = "full")]
        const _: () = {
            type Original = $original;
            $($boundary)*
            $($example)*

            impl poem_openapi::types::Type for Original {
                const IS_REQUIRED: bool = true;
                type RawValueType = Self;
                type RawElementValueType = Self;
                fn name() -> std::borrow::Cow<'static, str> {
                    <$name as poem_openapi::types::Type>::name()
                }
                fn schema_ref() -> poem_openapi::registry::MetaSchemaRef {
                    <$name as poem_openapi::types::Type>::schema_ref()
                }
                fn register(registry: &mut poem_openapi::registry::Registry) {
                    <$name as poem_openapi::types::Type>::register(registry)
                }
                fn as_raw_value(&self) -> Option<&Self> { Some(self) }
                fn raw_element_iter<'a>(&'a self) -> Box<dyn Iterator<Item = &'a Self> + 'a> {
                    Box::new(std::iter::once(self))
                }
            }
            impl poem_openapi::types::IsObjectType for Original {}
            impl poem_openapi::types::ParseFromJSON for Original {
                fn parse_from_json(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
                    let boundary = <$name as poem_openapi::types::ParseFromJSON>::parse_from_json(value)
                        .map_err(poem_openapi::types::ParseError::propagate)?;
                    let value = serde_json::to_value(boundary).map_err(poem_openapi::types::ParseError::custom)?;
                    serde_json::from_value(value).map_err(poem_openapi::types::ParseError::custom)
                }
            }
            impl poem_openapi::types::ToJSON for Original {
                fn to_json(&self) -> Option<serde_json::Value> {
                    let value = serde_json::to_value(self).expect("domain object must serialize");
                    let boundary: $name = serde_json::from_value(value).expect("OpenAPI boundary must match the domain object");
                    poem_openapi::types::ToJSON::to_json(&boundary)
                }
            }
            impl poem_openapi::types::ParseFromXML for Original {
                fn parse_from_xml(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
                    <Self as poem_openapi::types::ParseFromJSON>::parse_from_json(value)
                }
            }
            impl poem_openapi::types::ToXML for Original {
                fn to_xml(&self) -> Option<serde_json::Value> {
                    poem_openapi::types::ToJSON::to_json(self)
                }
            }
            impl poem_openapi::types::ParseFromYAML for Original {
                fn parse_from_yaml(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
                    <Self as poem_openapi::types::ParseFromJSON>::parse_from_json(value)
                }
            }
            impl poem_openapi::types::ToYAML for Original {
                fn to_yaml(&self) -> Option<serde_json::Value> {
                    poem_openapi::types::ToJSON::to_json(self)
                }
            }
        };
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_model::auth::TokenCreation;
    use poem_openapi::registry::Registry;
    use test_r::test;

    #[test]
    fn datetime_object_preserves_nanoseconds_and_normalizes_offsets() {
        let wire = serde_json::json!({"expiresAt": "1969-12-31T22:59:59.999999999-01:00"});
        let token = TokenCreation::parse_from_json(Some(wire)).unwrap();
        assert_eq!(token.expires_at.as_nanosecond(), -1);
        assert_eq!(
            token.to_json(),
            Some(serde_json::json!({"expiresAt": "1969-12-31T23:59:59.999999999Z"}))
        );
        let mut registry = Registry::new();
        TokenCreation::register(&mut registry);
        let schema = &registry.schemas["TokenCreation"];
        assert_eq!(schema.required, vec!["expiresAt"]);
        assert_eq!(schema.properties[0].1, OpenApiTimestamp::schema_ref());
    }

    #[test]
    fn datetime_object_rejects_missing_or_invalid_timestamp() {
        for value in [
            serde_json::json!({}),
            serde_json::json!({"expiresAt": null}),
            serde_json::json!({"expiresAt": "not-a-timestamp"}),
            serde_json::json!({"expiresAt": "2026-01-01T12:30Z"}),
            serde_json::json!({"expiresAt": "-000001-01-01T00:00:00Z"}),
            serde_json::json!({"expiresAt": "0000-01-01T00:00:00+01:00"}),
        ] {
            assert!(TokenCreation::parse_from_json(Some(value)).is_err());
        }
    }

    #[test]
    fn timestamp_parameters_preserve_nanoseconds_and_reject_invalid_values() {
        let value =
            OpenApiTimestamp::parse_from_parameter("1969-12-31T23:59:59.999999999Z").unwrap();
        assert_eq!(value.0.as_nanosecond(), -1);
        let year_zero =
            OpenApiTimestamp::parse_from_parameter("0000-01-01T01:00:00+01:00").unwrap();
        assert_eq!(year_zero.0.to_string(), "0000-01-01T00:00:00Z");
        assert_eq!(
            OpenApiTimestamp::parse_from_json(year_zero.to_json()).unwrap(),
            year_zero
        );
        for invalid in [
            "not-a-timestamp",
            "2026-01-01T12:30Z",
            "-000001-01-01T00:00:00Z",
            "0000-01-01T00:00:00+01:00",
        ] {
            assert!(OpenApiTimestamp::parse_from_parameter(invalid).is_err());
            assert!(
                serde_json::from_value::<OpenApiTimestamp>(serde_json::json!(invalid)).is_err()
            );
        }
    }
}
