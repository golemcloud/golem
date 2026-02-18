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

use poem::web::Field as PoemField;
use poem_openapi::registry::MetaSchema;
use poem_openapi::{
    registry::{MetaDiscriminatorObject, MetaSchemaRef, Registry},
    types::{ParseError, ParseFromJSON, ParseFromMultipartField, ParseResult, ToJSON, Type},
};
use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::borrow::Cow;
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum OptionalFieldUpdate<T> {
    Set(T),
    Unset,
    NoChange,
}

impl<T> OptionalFieldUpdate<T> {
    pub fn compute_new_value(self, old_value: Option<T>) -> Option<T> {
        match self {
            Self::Set(new_value) => Some(new_value),
            Self::Unset => None,
            Self::NoChange => old_value,
        }
    }

    pub fn update_from_option(value: Option<T>) -> Self {
        value.map(Self::Set).unwrap_or(Self::Unset)
    }

    pub fn is_no_change(&self) -> bool {
        matches!(self, OptionalFieldUpdate::NoChange)
    }
}

impl<T> Serialize for OptionalFieldUpdate<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            OptionalFieldUpdate::Set(value) => {
                let mut state = serializer.serialize_struct("FieldUpdate", 2)?;
                state.serialize_field("op", "set")?;
                state.serialize_field("value", value)?;
                state.end()
            }
            OptionalFieldUpdate::Unset => {
                let mut state = serializer.serialize_struct("FieldUpdate", 1)?;
                state.serialize_field("op", "unset")?;
                state.end()
            }
            OptionalFieldUpdate::NoChange => serializer.serialize_none(), // serializes as null
        }
    }
}

impl<'de, T> Deserialize<'de> for OptionalFieldUpdate<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<OptionalFieldUpdate<T>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FieldUpdateVisitor<T>(std::marker::PhantomData<T>);

        impl<'de, T> Visitor<'de> for FieldUpdateVisitor<T>
        where
            T: Deserialize<'de>,
        {
            type Value = OptionalFieldUpdate<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str(r#"{"op": "set", "value": ...} or {"op": "unset"} or null"#)
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(OptionalFieldUpdate::NoChange)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(OptionalFieldUpdate::NoChange)
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_map(self)
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut op: Option<String> = None;
                let mut value: Option<T> = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "op" => {
                            if op.is_some() {
                                return Err(de::Error::duplicate_field("op"));
                            }
                            op = Some(map.next_value()?);
                        }
                        "value" => {
                            if value.is_some() {
                                return Err(de::Error::duplicate_field("value"));
                            }
                            value = Some(map.next_value()?);
                        }
                        other => return Err(de::Error::unknown_field(other, &["op", "value"])),
                    }
                }

                match op.as_deref() {
                    Some("set") => {
                        let value = value.ok_or_else(|| de::Error::missing_field("value"))?;
                        Ok(OptionalFieldUpdate::Set(value))
                    }
                    Some("unset") => Ok(OptionalFieldUpdate::Unset),
                    Some(other) => Err(de::Error::unknown_variant(other, &["set", "unset"])),
                    None => Err(de::Error::missing_field("op")),
                }
            }
        }

        deserializer.deserialize_option(FieldUpdateVisitor(std::marker::PhantomData))
    }
}

impl<T: Type> Type for OptionalFieldUpdate<T> {
    const IS_REQUIRED: bool = false;

    type RawValueType = Self;
    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        format!("OptionalFieldUpdate_{}", T::name()).into()
    }

    fn schema_ref() -> MetaSchemaRef {
        MetaSchemaRef::Reference(Self::name().into_owned())
    }

    fn register(registry: &mut Registry) {
        T::register(registry);
        registry.create_schema::<Self, _>(Self::name().into_owned(), |registry| {
            let set_schema_name = format!("{}_Set", Self::name());
            let unset_schema_name = format!("{}_Unset", Self::name());

            registry.schemas.insert(
                set_schema_name.clone(),
                MetaSchema {
                    ty: "object",
                    required: vec!["op", "value"],
                    properties: vec![
                        (
                            "op",
                            MetaSchemaRef::Inline(Box::new(MetaSchema {
                                ty: "string",
                                enum_items: vec!["set".into()],
                                ..MetaSchema::ANY
                            })),
                        ),
                        ("value", MetaSchemaRef::Reference(T::name().into_owned())),
                    ],
                    ..MetaSchema::ANY
                },
            );

            registry.schemas.insert(
                unset_schema_name.clone(),
                MetaSchema {
                    ty: "object",
                    required: vec!["op"],
                    properties: vec![(
                        "op",
                        MetaSchemaRef::Inline(Box::new(MetaSchema {
                            ty: "string",
                            enum_items: vec!["set".into()],
                            ..MetaSchema::ANY
                        })),
                    )],
                    ..MetaSchema::ANY
                },
            );

            MetaSchema {
                ty: "object",
                one_of: vec![
                    MetaSchemaRef::Reference(set_schema_name.clone()),
                    MetaSchemaRef::Reference(unset_schema_name.clone()),
                ],
                discriminator: Some(MetaDiscriminatorObject {
                    property_name: "op",
                    mapping: vec![
                        (
                            "set".to_string(),
                            format!("#/components/schemas/{}", set_schema_name),
                        ),
                        (
                            "unset".to_string(),
                            format!("#/components/schemas/{}", unset_schema_name),
                        ),
                    ],
                }),
                ..MetaSchema::ANY
            }
        });
    }

    fn as_raw_value(&self) -> Option<&Self::RawValueType> {
        Some(self)
    }

    fn raw_element_iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
        Box::new(std::iter::once(self))
    }

    #[inline]
    fn is_none(&self) -> bool {
        matches!(self, OptionalFieldUpdate::NoChange)
    }
}

impl<T: ParseFromJSON> ParseFromJSON for OptionalFieldUpdate<T> {
    fn parse_from_json(value: Option<Value>) -> ParseResult<Self> {
        match value.unwrap_or(Value::Null) {
            Value::Null => Ok(OptionalFieldUpdate::NoChange),
            Value::Object(map) => {
                let op = map
                    .get("op")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ParseError::custom("Missing 'op' field"))?;

                match op {
                    "set" => {
                        let val = map
                            .get("value")
                            .ok_or_else(|| ParseError::custom("Missing 'value' field for set"))?;
                        T::parse_from_json(Some(val.clone()))
                            .map(OptionalFieldUpdate::Set)
                            .map_err(ParseError::propagate)
                    }
                    "unset" => Ok(OptionalFieldUpdate::Unset),
                    other => Err(ParseError::custom(format!("Unknown op '{}'", other))),
                }
            }
            other => Err(ParseError::custom(format!(
                "Expected object or null, got {:?}",
                other
            ))),
        }
    }
}

impl<T: ToJSON> ToJSON for OptionalFieldUpdate<T> {
    fn to_json(&self) -> Option<Value> {
        match self {
            OptionalFieldUpdate::Set(value) => {
                let mut obj = serde_json::Map::new();
                obj.insert("op".to_string(), Value::String("set".to_string()));
                obj.insert("value".to_string(), value.to_json().unwrap_or(Value::Null));
                Some(Value::Object(obj))
            }
            OptionalFieldUpdate::Unset => {
                let mut obj = serde_json::Map::new();
                obj.insert("op".to_string(), Value::String("unset".to_string()));
                Some(Value::Object(obj))
            }
            OptionalFieldUpdate::NoChange => Some(Value::Null),
        }
    }
}

impl<T: ParseFromMultipartField> ParseFromMultipartField for OptionalFieldUpdate<T> {
    async fn parse_from_multipart(value: Option<PoemField>) -> ParseResult<Self> {
        match value {
            Some(field) => {
                let val = T::parse_from_multipart(Some(field))
                    .await
                    .map(OptionalFieldUpdate::Set)
                    .map_err(ParseError::propagate)?;
                Ok(val)
            }
            None => Ok(OptionalFieldUpdate::NoChange),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;
    use test_r::test;

    #[test]
    fn serialize_set() {
        let value = OptionalFieldUpdate::Set("foo".to_string());
        let json = serde_json::to_string(&value).unwrap();
        let expected = r#"{"op":"set","value":"foo"}"#;
        assert_eq!(json, expected);
    }

    #[test]
    fn serialize_unset() {
        let value = OptionalFieldUpdate::<String>::Unset;
        let json = serde_json::to_string(&value).unwrap();
        let expected = r#"{"op":"unset"}"#;
        assert_eq!(json, expected);
    }

    #[test]
    fn serialize_nochange() {
        let value = OptionalFieldUpdate::<String>::NoChange;
        let json = serde_json::to_string(&value).unwrap();
        let expected = "null";
        assert_eq!(json, expected);
    }

    #[test]
    fn deserialize_set() {
        let json = r#"{"op":"set","value":"foo"}"#;
        let value: OptionalFieldUpdate<String> = serde_json::from_str(json).unwrap();
        assert_eq!(value, OptionalFieldUpdate::Set("foo".to_string()));
    }

    #[test]
    fn deserialize_unset() {
        let json = r#"{"op":"unset"}"#;
        let value: OptionalFieldUpdate<String> = serde_json::from_str(json).unwrap();
        assert_eq!(value, OptionalFieldUpdate::Unset);
    }

    #[test]
    fn deserialize_nochange() {
        let json = "null";
        let value: OptionalFieldUpdate<String> = serde_json::from_str(json).unwrap();
        assert_eq!(value, OptionalFieldUpdate::NoChange);
    }

    #[test]
    fn round_trip_set() {
        let original = OptionalFieldUpdate::Set("bar".to_string());
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: OptionalFieldUpdate<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(original, deserialized);
    }

    #[test]
    fn round_trip_unset() {
        let original = OptionalFieldUpdate::<String>::Unset;
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: OptionalFieldUpdate<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(original, deserialized);
    }

    #[test]
    fn round_trip_nochange() {
        let original = OptionalFieldUpdate::<String>::NoChange;
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: OptionalFieldUpdate<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(original, deserialized);
    }

    #[test]
    fn invalid_op() {
        let json = r#"{"op":"invalid"}"#;
        let result: Result<OptionalFieldUpdate<String>, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn missing_value_for_set() {
        let json = r#"{"op":"set"}"#;
        let result: Result<OptionalFieldUpdate<String>, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }
}
