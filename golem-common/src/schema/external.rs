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

use crate::schema::{SchemaValue, TypedSchemaValue, find_host_managed_value};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Canonical schema-value JSON that cannot contain a host-managed capability.
#[derive(Clone, Debug, PartialEq)]
pub struct ExternalSchemaValue(SchemaValue);

impl ExternalSchemaValue {
    pub fn as_inner(&self) -> &SchemaValue {
        &self.0
    }

    pub fn into_inner(self) -> SchemaValue {
        self.0
    }
}

impl TryFrom<SchemaValue> for ExternalSchemaValue {
    type Error = String;

    fn try_from(value: SchemaValue) -> Result<Self, Self::Error> {
        match find_host_managed_value(&value) {
            Some(occurrence) => Err(format!(
                "host-managed capability `{}` cannot cross an external JSON boundary ({})",
                occurrence.kind.kind_name(),
                occurrence.path
            )),
            None => Ok(Self(value)),
        }
    }
}

impl From<ExternalSchemaValue> for SchemaValue {
    fn from(value: ExternalSchemaValue) -> Self {
        value.into_inner()
    }
}

impl Serialize for ExternalSchemaValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ExternalSchemaValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = SchemaValue::deserialize(deserializer)?;
        Self::try_from(value).map_err(serde::de::Error::custom)
    }
}

/// A typed canonical value whose value tree is safe for external JSON.
#[derive(Clone, Debug, PartialEq)]
pub struct ExternalTypedSchemaValue(TypedSchemaValue);

impl ExternalTypedSchemaValue {
    pub fn as_inner(&self) -> &TypedSchemaValue {
        &self.0
    }

    pub fn into_inner(self) -> TypedSchemaValue {
        self.0
    }
}

impl TryFrom<TypedSchemaValue> for ExternalTypedSchemaValue {
    type Error = String;

    fn try_from(value: TypedSchemaValue) -> Result<Self, Self::Error> {
        match find_host_managed_value(value.value()) {
            Some(occurrence) => Err(format!(
                "host-managed capability `{}` cannot cross an external JSON boundary ({})",
                occurrence.kind.kind_name(),
                occurrence.path
            )),
            None => Ok(Self(value)),
        }
    }
}

impl From<ExternalTypedSchemaValue> for TypedSchemaValue {
    fn from(value: ExternalTypedSchemaValue) -> Self {
        value.into_inner()
    }
}

impl Serialize for ExternalTypedSchemaValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ExternalTypedSchemaValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = TypedSchemaValue::deserialize(deserializer)?;
        Self::try_from(value).map_err(serde::de::Error::custom)
    }
}

#[cfg(feature = "full")]
mod poem_impl {
    use super::{ExternalSchemaValue, ExternalTypedSchemaValue};
    use crate::schema::{SchemaValue, TypedSchemaValue};
    use poem_openapi::registry::{MetaSchemaRef, Registry};
    use poem_openapi::types::{ParseError, ParseFromJSON, ParseResult, ToJSON, Type};
    use serde_json::Value;
    use std::borrow::Cow;

    #[allow(dead_code)]
    mod openapi_schema {
        use crate::schema::{
            BinaryValuePayload, DurationValuePayload, QuantityValue, SchemaGraph, TextValuePayload,
        };
        use chrono::{DateTime, Utc};

        #[derive(serde::Serialize, serde::Deserialize, golem_schema_derive::PoemSchema)]
        #[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
        pub enum ExternalSchemaValue {
            Bool(bool),
            S8(i8),
            S16(i16),
            S32(i32),
            S64(i64),
            U8(u8),
            U16(u16),
            U32(u32),
            U64(u64),
            F32(f32),
            F64(f64),
            Char(char),
            String(String),
            Record {
                fields: Vec<ExternalSchemaValue>,
            },
            Variant(ExternalVariantValuePayload),
            Enum {
                case: u32,
            },
            Flags {
                bits: Vec<bool>,
            },
            Tuple {
                elements: Vec<ExternalSchemaValue>,
            },
            List {
                elements: Vec<ExternalSchemaValue>,
            },
            FixedList {
                elements: Vec<ExternalSchemaValue>,
            },
            Map {
                entries: Vec<(ExternalSchemaValue, ExternalSchemaValue)>,
            },
            Option {
                inner: Option<Box<ExternalSchemaValue>>,
            },
            Result(ExternalResultValuePayload),
            Text(TextValuePayload),
            Binary(BinaryValuePayload),
            Path {
                path: String,
            },
            Url {
                url: String,
            },
            Datetime {
                value: DateTime<Utc>,
            },
            Duration(DurationValuePayload),
            Quantity(QuantityValue),
            Union(ExternalUnionValuePayload),
        }

        #[derive(serde::Serialize, serde::Deserialize, golem_schema_derive::PoemSchema)]
        #[serde(rename_all = "camelCase")]
        pub struct ExternalVariantValuePayload {
            pub case: u32,
            pub payload: Option<Box<ExternalSchemaValue>>,
        }

        #[derive(serde::Serialize, serde::Deserialize, golem_schema_derive::PoemSchema)]
        #[serde(tag = "tag", rename_all = "kebab-case")]
        pub enum ExternalResultValuePayload {
            Ok {
                value: Option<Box<ExternalSchemaValue>>,
            },
            Err {
                value: Option<Box<ExternalSchemaValue>>,
            },
        }

        #[derive(serde::Serialize, serde::Deserialize, golem_schema_derive::PoemSchema)]
        #[serde(rename_all = "camelCase")]
        pub struct ExternalUnionValuePayload {
            pub tag: String,
            pub body: Box<ExternalSchemaValue>,
        }

        #[derive(serde::Serialize, serde::Deserialize, golem_schema_derive::PoemSchema)]
        #[serde(rename_all = "camelCase")]
        pub struct ExternalTypedSchemaValue {
            pub graph: SchemaGraph,
            pub value: ExternalSchemaValue,
        }
    }

    macro_rules! impl_external_poem_type {
        ($external:ty, $inner:ty, $schema:ty, $name:literal) => {
            impl Type for $external {
                const IS_REQUIRED: bool = true;
                type RawValueType = Self;
                type RawElementValueType = Self;

                fn name() -> Cow<'static, str> {
                    $name.into()
                }

                fn schema_ref() -> MetaSchemaRef {
                    <$schema as Type>::schema_ref()
                }

                fn register(registry: &mut Registry) {
                    <$schema as Type>::register(registry);
                }

                fn as_raw_value(&self) -> Option<&Self::RawValueType> {
                    Some(self)
                }

                fn raw_element_iter<'a>(
                    &'a self,
                ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
                    Box::new(std::iter::once(self))
                }
            }

            impl ParseFromJSON for $external {
                fn parse_from_json(value: Option<Value>) -> ParseResult<Self> {
                    <$inner as ParseFromJSON>::parse_from_json(value)
                        .map_err(ParseError::propagate)
                        .and_then(|value| Self::try_from(value).map_err(ParseError::custom))
                }
            }

            impl ToJSON for $external {
                fn to_json(&self) -> Option<Value> {
                    self.as_inner().to_json()
                }
            }
        };
    }

    impl_external_poem_type!(
        ExternalSchemaValue,
        SchemaValue,
        openapi_schema::ExternalSchemaValue,
        "ExternalSchemaValue"
    );
    impl_external_poem_type!(
        ExternalTypedSchemaValue,
        TypedSchemaValue,
        openapi_schema::ExternalTypedSchemaValue,
        "ExternalTypedSchemaValue"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{QuotaTokenValuePayload, SchemaGraph};
    use chrono::{TimeZone, Utc};
    use test_r::test;

    fn forged_quota_token() -> SchemaValue {
        SchemaValue::QuotaToken(QuotaTokenValuePayload {
            environment_id: golem_schema::EnvironmentId::new(uuid::Uuid::nil()),
            resource_name: "forged".to_string(),
            expected_use: 100,
            last_credit: 100,
            last_credit_at: Utc.timestamp_opt(0, 0).unwrap(),
        })
    }

    #[test]
    fn canonical_external_value_rejects_nested_capabilities() {
        let value = SchemaValue::Record {
            fields: vec![SchemaValue::Option {
                inner: Some(Box::new(forged_quota_token())),
            }],
        };
        let json = serde_json::to_value(value).unwrap();

        let error = serde_json::from_value::<ExternalSchemaValue>(json).unwrap_err();
        assert!(error.to_string().contains("quota-token"));
    }

    #[test]
    fn typed_external_value_rejects_capabilities_but_preserves_ordinary_values() {
        let forged = TypedSchemaValue::new(
            SchemaGraph::anonymous(crate::schema::SchemaType::quota_token(Default::default())),
            forged_quota_token(),
        );
        assert!(ExternalTypedSchemaValue::try_from(forged).is_err());

        let ordinary = TypedSchemaValue::new(
            SchemaGraph::anonymous(crate::schema::SchemaType::string()),
            SchemaValue::String("safe".to_string()),
        );
        let external = ExternalTypedSchemaValue::try_from(ordinary.clone()).unwrap();
        assert_eq!(external.into_inner(), ordinary);
    }

    #[cfg(feature = "full")]
    #[test]
    fn external_openapi_schema_is_recursive_without_capability_value_variants() {
        use poem_openapi::registry::Registry;
        use poem_openapi::types::Type;

        let mut registry = Registry::new();
        ExternalTypedSchemaValue::register(&mut registry);
        let value_schema = serde_json::to_string(
            registry
                .schemas
                .get("ExternalSchemaValue")
                .expect("external value schema must be registered"),
        )
        .unwrap();
        let typed_schema = serde_json::to_string(
            registry
                .schemas
                .get("ExternalTypedSchemaValue")
                .expect("external typed-value schema must be registered"),
        )
        .unwrap();

        assert!(value_schema.contains("ExternalSchemaValue"));
        assert!(!value_schema.contains("SecretValuePayload"));
        assert!(!value_schema.contains("QuotaTokenValuePayload"));
        assert!(!value_schema.contains("PermissionCardValuePayload"));
        assert!(typed_schema.contains("ExternalSchemaValue"));
        assert!(registry.schemas.contains_key("SchemaType"));
        assert!(registry.schemas.contains_key("SecretSpec"));
    }
}
