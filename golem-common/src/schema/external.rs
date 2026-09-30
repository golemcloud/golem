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

use crate::schema::{
    BinaryValuePayload, DurationValuePayload, QuantityValue, ResultValuePayload, SchemaValue,
    TextValuePayload, TypedSchemaValue, UnionValuePayload, VariantValuePayload,
    find_host_managed_value,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Number, Value};

const EXCEPTIONAL_FLOAT_KEY: &str = "$float";

fn exact_object<'a>(value: &'a Value, fields: &[&str]) -> Result<&'a Map<String, Value>, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "expected a JSON object".to_string())?;
    if object.len() != fields.len()
        || object
            .keys()
            .any(|key| !fields.iter().any(|field| key == field))
    {
        return Err("object members do not match the expected representation".to_string());
    }
    Ok(object)
}

fn required<'a>(object: &'a Map<String, Value>, field: &str) -> Result<&'a Value, String> {
    object
        .get(field)
        .ok_or_else(|| format!("missing field `{field}`"))
}

fn canonical_unsigned(value: &str) -> bool {
    !value.is_empty()
        && (value == "0" || !value.starts_with('0'))
        && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn canonical_signed(value: &str) -> bool {
    if let Some(rest) = value.strip_prefix('-') {
        rest != "0" && canonical_unsigned(rest)
    } else {
        canonical_unsigned(value)
    }
}

fn parse_s64(value: &Value) -> Result<i64, String> {
    let value = value
        .as_str()
        .ok_or_else(|| "s64 must be a canonical decimal string".to_string())?;
    if !canonical_signed(value) {
        return Err("s64 must be a canonical decimal string".to_string());
    }
    value.parse().map_err(|_| "s64 is out of range".to_string())
}

fn parse_u64(value: &Value) -> Result<u64, String> {
    let value = value
        .as_str()
        .ok_or_else(|| "u64 must be a canonical decimal string".to_string())?;
    if !canonical_unsigned(value) {
        return Err("u64 must be a canonical decimal string".to_string());
    }
    value.parse().map_err(|_| "u64 is out of range".to_string())
}

fn encode_float(value: f64) -> Result<Value, String> {
    if value.is_nan() {
        return Ok(serde_json::json!({ EXCEPTIONAL_FLOAT_KEY: "nan" }));
    }
    if value == f64::INFINITY {
        return Ok(serde_json::json!({ EXCEPTIONAL_FLOAT_KEY: "positive-infinity" }));
    }
    if value == f64::NEG_INFINITY {
        return Ok(serde_json::json!({ EXCEPTIONAL_FLOAT_KEY: "negative-infinity" }));
    }
    Number::from_f64(value)
        .map(Value::Number)
        .ok_or_else(|| "invalid finite float".to_string())
}

fn encode_f32(value: f32) -> Result<Value, String> {
    if value.is_finite() {
        value
            .to_string()
            .parse::<Number>()
            .map(Value::Number)
            .map_err(|_| "invalid finite f32".to_string())
    } else {
        encode_float(value as f64)
    }
}

fn parse_float(value: &Value) -> Result<f64, String> {
    if let Some(value) = value.as_f64() {
        return Ok(value);
    }
    let object = exact_object(value, &[EXCEPTIONAL_FLOAT_KEY])?;
    match required(object, EXCEPTIONAL_FLOAT_KEY)?.as_str() {
        Some("nan") => Ok(f64::NAN),
        Some("positive-infinity") => Ok(f64::INFINITY),
        Some("negative-infinity") => Ok(f64::NEG_INFINITY),
        _ => Err("unknown exceptional float representation".to_string()),
    }
}

pub(crate) fn encode_external_schema_value(value: &SchemaValue) -> Result<Value, String> {
    let payload = match value {
        SchemaValue::Bool(value) => ("bool", Value::Bool(*value)),
        SchemaValue::S8(value) => ("s8", Value::Number((*value).into())),
        SchemaValue::S16(value) => ("s16", Value::Number((*value).into())),
        SchemaValue::S32(value) => ("s32", Value::Number((*value).into())),
        SchemaValue::S64(value) => ("s64", Value::String(value.to_string())),
        SchemaValue::U8(value) => ("u8", Value::Number((*value).into())),
        SchemaValue::U16(value) => ("u16", Value::Number((*value).into())),
        SchemaValue::U32(value) => ("u32", Value::Number((*value).into())),
        SchemaValue::U64(value) => ("u64", Value::String(value.to_string())),
        SchemaValue::F32(value) => ("f32", encode_f32(*value)?),
        SchemaValue::F64(value) => ("f64", encode_float(*value)?),
        SchemaValue::Char(value) => ("char", Value::String(value.to_string())),
        SchemaValue::String(value) => ("string", Value::String(value.clone())),
        SchemaValue::Uuid(value) => ("uuid", Value::String(value.hyphenated().to_string())),
        SchemaValue::Record { fields } => (
            "record",
            serde_json::json!({
                "fields": fields.iter().map(encode_external_schema_value).collect::<Result<Vec<_>, _>>()?
            }),
        ),
        SchemaValue::Variant(value) => {
            let mut payload = Map::new();
            payload.insert("case".to_string(), Value::Number(value.case.into()));
            if let Some(value) = &value.payload {
                payload.insert("payload".to_string(), encode_external_schema_value(value)?);
            }
            ("variant", Value::Object(payload))
        }
        SchemaValue::Enum { case } => ("enum", serde_json::json!({ "case": case })),
        SchemaValue::Flags { bits } => ("flags", serde_json::json!({ "bits": bits })),
        SchemaValue::Tuple { elements } => (
            "tuple",
            serde_json::json!({
                "elements": elements.iter().map(encode_external_schema_value).collect::<Result<Vec<_>, _>>()?
            }),
        ),
        SchemaValue::List { elements } => (
            "list",
            serde_json::json!({
                "elements": elements.iter().map(encode_external_schema_value).collect::<Result<Vec<_>, _>>()?
            }),
        ),
        SchemaValue::FixedList { elements } => (
            "fixed-list",
            serde_json::json!({
                "elements": elements.iter().map(encode_external_schema_value).collect::<Result<Vec<_>, _>>()?
            }),
        ),
        SchemaValue::Map { entries } => (
            "map",
            serde_json::json!({
                "entries": entries
                    .iter()
                    .map(|(key, value)| Ok([encode_external_schema_value(key)?, encode_external_schema_value(value)?]))
                    .collect::<Result<Vec<_>, String>>()?
            }),
        ),
        SchemaValue::Option { inner } => (
            "option",
            serde_json::json!({
                "inner": inner.as_deref().map(encode_external_schema_value).transpose()?
            }),
        ),
        SchemaValue::Result(result) => {
            let (tag, value) = match result {
                ResultValuePayload::Ok { value } => ("ok", value),
                ResultValuePayload::Err { value } => ("err", value),
            };
            (
                "result",
                serde_json::json!({
                    "tag": tag,
                    "value": value.as_deref().map(encode_external_schema_value).transpose()?
                }),
            )
        }
        SchemaValue::Text(value) => {
            let mut payload = Map::new();
            payload.insert("text".to_string(), Value::String(value.text.clone()));
            if let Some(language) = &value.language {
                payload.insert("language".to_string(), Value::String(language.clone()));
            }
            ("text", Value::Object(payload))
        }
        SchemaValue::Binary(value) => {
            let mut payload = Map::new();
            payload.insert(
                "bytes".to_string(),
                Value::Array(
                    value
                        .bytes
                        .iter()
                        .map(|byte| Value::Number((*byte).into()))
                        .collect(),
                ),
            );
            if let Some(mime_type) = &value.mime_type {
                payload.insert("mimeType".to_string(), Value::String(mime_type.clone()));
            }
            ("binary", Value::Object(payload))
        }
        SchemaValue::Path { path } => ("path", serde_json::json!({ "path": path })),
        SchemaValue::Url { url } => ("url", serde_json::json!({ "url": url })),
        SchemaValue::Datetime { value } => ("datetime", serde_json::json!({ "value": value })),
        SchemaValue::Duration(value) => (
            "duration",
            serde_json::json!({ "nanoseconds": value.nanoseconds.to_string() }),
        ),
        SchemaValue::Quantity(value) => (
            "quantity",
            serde_json::json!({
                "mantissa": value.mantissa.to_string(),
                "scale": value.scale,
                "unit": value.unit,
            }),
        ),
        SchemaValue::Union(value) => (
            "union",
            serde_json::json!({
                "tag": value.tag,
                "body": encode_external_schema_value(&value.body)?,
            }),
        ),
        SchemaValue::Secret(_)
        | SchemaValue::QuotaToken(_)
        | SchemaValue::PermissionCard(_)
        | SchemaValue::Stream(_) => {
            return Err("host-managed capabilities cannot cross an external JSON boundary".into());
        }
    };
    Ok(serde_json::json!({ "kind": payload.0, "value": payload.1 }))
}

fn object_with_optional<'a>(
    value: &'a Value,
    required_fields: &[&str],
    optional_fields: &[&str],
) -> Result<&'a Map<String, Value>, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "expected a JSON object".to_string())?;
    if required_fields
        .iter()
        .any(|field| !object.contains_key(*field))
        || object.keys().any(|key| {
            !required_fields.iter().any(|field| key == field)
                && !optional_fields.iter().any(|field| key == field)
        })
    {
        return Err("object members do not match the expected representation".to_string());
    }
    Ok(object)
}

fn parse_signed<T>(value: &Value, kind: &str) -> Result<T, String>
where
    T: TryFrom<i64>,
{
    value
        .as_i64()
        .and_then(|value| value.try_into().ok())
        .ok_or_else(|| format!("{kind} must be an in-range JSON integer"))
}

fn parse_unsigned<T>(value: &Value, kind: &str) -> Result<T, String>
where
    T: TryFrom<u64>,
{
    value
        .as_u64()
        .and_then(|value| value.try_into().ok())
        .ok_or_else(|| format!("{kind} must be an in-range JSON integer"))
}

fn parse_values(value: &Value, field: &str) -> Result<Vec<SchemaValue>, String> {
    value
        .as_array()
        .ok_or_else(|| format!("{field} must be an array"))?
        .iter()
        .map(decode_external_schema_value)
        .collect()
}

pub(crate) fn decode_external_schema_value(value: &Value) -> Result<SchemaValue, String> {
    let outer = exact_object(value, &["kind", "value"])?;
    let kind = required(outer, "kind")?
        .as_str()
        .ok_or_else(|| "kind must be a string".to_string())?;
    let value = required(outer, "value")?;

    match kind {
        "bool" => value
            .as_bool()
            .map(SchemaValue::Bool)
            .ok_or_else(|| "bool value must be a boolean".to_string()),
        "s8" => parse_signed(value, kind).map(SchemaValue::S8),
        "s16" => parse_signed(value, kind).map(SchemaValue::S16),
        "s32" => parse_signed(value, kind).map(SchemaValue::S32),
        "s64" => parse_s64(value).map(SchemaValue::S64),
        "u8" => parse_unsigned(value, kind).map(SchemaValue::U8),
        "u16" => parse_unsigned(value, kind).map(SchemaValue::U16),
        "u32" => parse_unsigned(value, kind).map(SchemaValue::U32),
        "u64" => parse_u64(value).map(SchemaValue::U64),
        "f32" => {
            let value = parse_float(value)?;
            let narrowed = value as f32;
            if narrowed.is_infinite() && value.is_finite() {
                Err("f32 is out of range".to_string())
            } else {
                Ok(SchemaValue::F32(narrowed))
            }
        }
        "f64" => parse_float(value).map(SchemaValue::F64),
        "char" => {
            let mut chars = value
                .as_str()
                .ok_or_else(|| "char value must be a string".to_string())?
                .chars();
            let character = chars
                .next()
                .ok_or_else(|| "char value must contain one character".to_string())?;
            if chars.next().is_some() {
                Err("char value must contain one character".to_string())
            } else {
                Ok(SchemaValue::Char(character))
            }
        }
        "string" => value
            .as_str()
            .map(|value| SchemaValue::String(value.to_string()))
            .ok_or_else(|| "string value must be a string".to_string()),
        "uuid" => {
            let encoded = value
                .as_str()
                .ok_or_else(|| "uuid value must be a string".to_string())?;
            let uuid = encoded
                .parse::<uuid::Uuid>()
                .map_err(|error| format!("invalid uuid: {error}"))?;
            if uuid.hyphenated().to_string() != encoded {
                Err("uuid must be a canonical lowercase hyphenated string".to_string())
            } else {
                Ok(SchemaValue::Uuid(uuid))
            }
        }
        "record" => {
            let object = exact_object(value, &["fields"])?;
            Ok(SchemaValue::Record {
                fields: parse_values(required(object, "fields")?, "record fields")?,
            })
        }
        "variant" => {
            let object = object_with_optional(value, &["case"], &["payload"])?;
            Ok(SchemaValue::Variant(VariantValuePayload {
                case: parse_unsigned(required(object, "case")?, "variant case")?,
                payload: object
                    .get("payload")
                    .map(decode_external_schema_value)
                    .transpose()?
                    .map(Box::new),
            }))
        }
        "enum" => {
            let object = exact_object(value, &["case"])?;
            Ok(SchemaValue::Enum {
                case: parse_unsigned(required(object, "case")?, "enum case")?,
            })
        }
        "flags" => {
            let object = exact_object(value, &["bits"])?;
            let bits = required(object, "bits")?
                .as_array()
                .ok_or_else(|| "flag bits must be an array".to_string())?
                .iter()
                .map(|value| {
                    value
                        .as_bool()
                        .ok_or_else(|| "flag bit must be a boolean".to_string())
                })
                .collect::<Result<_, _>>()?;
            Ok(SchemaValue::Flags { bits })
        }
        "tuple" | "list" | "fixed-list" => {
            let object = exact_object(value, &["elements"])?;
            let elements = parse_values(required(object, "elements")?, "elements")?;
            Ok(match kind {
                "tuple" => SchemaValue::Tuple { elements },
                "list" => SchemaValue::List { elements },
                _ => SchemaValue::FixedList { elements },
            })
        }
        "map" => {
            let object = exact_object(value, &["entries"])?;
            let entries = required(object, "entries")?
                .as_array()
                .ok_or_else(|| "map entries must be an array".to_string())?
                .iter()
                .map(|entry| {
                    let entry = entry
                        .as_array()
                        .filter(|entry| entry.len() == 2)
                        .ok_or_else(|| "map entry must contain a key and value".to_string())?;
                    Ok((
                        decode_external_schema_value(&entry[0])?,
                        decode_external_schema_value(&entry[1])?,
                    ))
                })
                .collect::<Result<_, String>>()?;
            Ok(SchemaValue::Map { entries })
        }
        "option" => {
            let object = exact_object(value, &["inner"])?;
            let inner = required(object, "inner")?;
            Ok(SchemaValue::Option {
                inner: if inner.is_null() {
                    None
                } else {
                    Some(Box::new(decode_external_schema_value(inner)?))
                },
            })
        }
        "result" => {
            let object = exact_object(value, &["tag", "value"])?;
            let payload = required(object, "value")?;
            let payload = if payload.is_null() {
                None
            } else {
                Some(Box::new(decode_external_schema_value(payload)?))
            };
            match required(object, "tag")?.as_str() {
                Some("ok") => Ok(SchemaValue::Result(ResultValuePayload::Ok {
                    value: payload,
                })),
                Some("err") => Ok(SchemaValue::Result(ResultValuePayload::Err {
                    value: payload,
                })),
                _ => Err("result tag must be `ok` or `err`".to_string()),
            }
        }
        "text" => {
            let object = object_with_optional(value, &["text"], &["language"])?;
            Ok(SchemaValue::Text(TextValuePayload {
                text: required(object, "text")?
                    .as_str()
                    .ok_or_else(|| "text must be a string".to_string())?
                    .to_string(),
                language: object
                    .get("language")
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_string)
                            .ok_or_else(|| "language must be a string".to_string())
                    })
                    .transpose()?,
            }))
        }
        "binary" => {
            let object = object_with_optional(value, &["bytes"], &["mimeType"])?;
            let bytes = required(object, "bytes")?
                .as_array()
                .ok_or_else(|| "binary bytes must be an array".to_string())?
                .iter()
                .map(|value| parse_unsigned(value, "byte"))
                .collect::<Result<_, _>>()?;
            let mime_type = object
                .get("mimeType")
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_string)
                        .ok_or_else(|| "MIME type must be a string".to_string())
                })
                .transpose()?;
            Ok(SchemaValue::Binary(BinaryValuePayload { bytes, mime_type }))
        }
        "path" | "url" => {
            let field = kind;
            let object = exact_object(value, &[field])?;
            let value = required(object, field)?
                .as_str()
                .ok_or_else(|| format!("{field} must be a string"))?
                .to_string();
            Ok(if kind == "path" {
                SchemaValue::Path { path: value }
            } else {
                SchemaValue::Url { url: value }
            })
        }
        "datetime" => {
            let object = exact_object(value, &["value"])?;
            let value = serde_json::from_value(required(object, "value")?.clone())
                .map_err(|error| format!("invalid datetime: {error}"))?;
            Ok(SchemaValue::Datetime { value })
        }
        "duration" => {
            let object = exact_object(value, &["nanoseconds"])?;
            Ok(SchemaValue::Duration(DurationValuePayload {
                nanoseconds: parse_s64(required(object, "nanoseconds")?)?,
            }))
        }
        "quantity" => {
            let object = exact_object(value, &["mantissa", "scale", "unit"])?;
            Ok(SchemaValue::Quantity(QuantityValue {
                mantissa: parse_s64(required(object, "mantissa")?)?,
                scale: parse_signed(required(object, "scale")?, "quantity scale")?,
                unit: required(object, "unit")?
                    .as_str()
                    .ok_or_else(|| "quantity unit must be a string".to_string())?
                    .to_string(),
            }))
        }
        "union" => {
            let object = exact_object(value, &["tag", "body"])?;
            Ok(SchemaValue::Union(UnionValuePayload {
                tag: required(object, "tag")?
                    .as_str()
                    .ok_or_else(|| "union tag must be a string".to_string())?
                    .to_string(),
                body: Box::new(decode_external_schema_value(required(object, "body")?)?),
            }))
        }
        _ => Err(format!("unknown external schema value kind `{kind}`")),
    }
}

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
        encode_external_schema_value(&self.0)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ExternalSchemaValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let value = decode_external_schema_value(&value).map_err(serde::de::Error::custom)?;
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
        let mut object = Map::new();
        object.insert(
            "graph".to_string(),
            serde_json::to_value(self.0.graph()).map_err(serde::ser::Error::custom)?,
        );
        object.insert(
            "value".to_string(),
            encode_external_schema_value(self.0.value()).map_err(serde::ser::Error::custom)?,
        );
        Value::Object(object).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ExternalTypedSchemaValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let object = exact_object(&value, &["graph", "value"]).map_err(serde::de::Error::custom)?;
        let graph = serde_json::from_value(required(object, "graph").unwrap().clone())
            .map_err(serde::de::Error::custom)?;
        let value = decode_external_schema_value(required(object, "value").unwrap())
            .map_err(serde::de::Error::custom)?;
        let value = TypedSchemaValue::new(graph, value);
        Self::try_from(value).map_err(serde::de::Error::custom)
    }
}

#[cfg(feature = "full")]
mod poem_impl {
    use super::{ExternalSchemaValue, ExternalTypedSchemaValue};
    use poem_openapi::registry::{MetaSchemaRef, Registry};
    use poem_openapi::types::{ParseError, ParseFromJSON, ParseResult, ToJSON, Type};
    use serde_json::Value;
    use std::borrow::Cow;

    #[allow(dead_code)]
    mod openapi_schema {
        use crate::schema::SchemaGraph;
        use chrono::{DateTime, Utc};
        use poem_openapi::registry::{MetaSchema, MetaSchemaRef, Registry};
        use poem_openapi::types::Type;
        use std::borrow::Cow;

        #[derive(serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct ExternalFloat(serde_json::Value);

        impl Type for ExternalFloat {
            const IS_REQUIRED: bool = true;
            type RawValueType = Self;
            type RawElementValueType = Self;

            fn name() -> Cow<'static, str> {
                "ExternalFloat".into()
            }

            fn schema_ref() -> MetaSchemaRef {
                MetaSchemaRef::Inline(Box::new(MetaSchema {
                    one_of: vec![
                        MetaSchemaRef::Inline(Box::new(MetaSchema::new("number"))),
                        MetaSchemaRef::Inline(Box::new(MetaSchema {
                            ty: "object",
                            required: vec!["$float"],
                            properties: vec![(
                                "$float",
                                MetaSchemaRef::Inline(Box::new(MetaSchema {
                                    ty: "string",
                                    enum_items: vec![
                                        "nan".into(),
                                        "positive-infinity".into(),
                                        "negative-infinity".into(),
                                    ],
                                    ..MetaSchema::ANY
                                })),
                            )],
                            ..MetaSchema::ANY
                        })),
                    ],
                    ..MetaSchema::ANY
                }))
            }

            fn register(_registry: &mut Registry) {}

            fn as_raw_value(&self) -> Option<&Self::RawValueType> {
                Some(self)
            }

            fn raw_element_iter<'a>(
                &'a self,
            ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
                Box::new(std::iter::once(self))
            }
        }

        #[derive(serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct OptionalNonNull<T>(Option<T>);

        impl<T> Default for OptionalNonNull<T> {
            fn default() -> Self {
                Self(None)
            }
        }

        impl<T> OptionalNonNull<T> {
            fn is_none(&self) -> bool {
                self.0.is_none()
            }
        }

        impl<T: Type> Type for OptionalNonNull<T> {
            const IS_REQUIRED: bool = false;
            type RawValueType = Self;
            type RawElementValueType = Self;

            fn name() -> Cow<'static, str> {
                format!("OptionalNonNull_{}", T::name()).into()
            }

            fn schema_ref() -> MetaSchemaRef {
                T::schema_ref()
            }

            fn register(registry: &mut Registry) {
                T::register(registry);
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

        #[derive(serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct RequiredNullable<T>(Option<T>);

        impl<T: Type> Type for RequiredNullable<T> {
            const IS_REQUIRED: bool = true;
            type RawValueType = Self;
            type RawElementValueType = Self;

            fn name() -> Cow<'static, str> {
                format!("RequiredNullable_{}", T::name()).into()
            }

            fn schema_ref() -> MetaSchemaRef {
                MetaSchemaRef::Inline(Box::new(MetaSchema {
                    one_of: vec![
                        T::schema_ref(),
                        MetaSchemaRef::Inline(Box::new(MetaSchema {
                            ty: "object",
                            nullable: true,
                            enum_items: vec![serde_json::Value::Null],
                            ..MetaSchema::ANY
                        })),
                    ],
                    ..MetaSchema::ANY
                }))
            }

            fn register(registry: &mut Registry) {
                T::register(registry);
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

        #[derive(serde::Serialize, serde::Deserialize, golem_schema_derive::PoemSchema)]
        #[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
        pub enum ExternalSchemaValue {
            Bool(bool),
            S8(i8),
            S16(i16),
            S32(i32),
            S64(String),
            U8(u8),
            U16(u16),
            U32(u32),
            U64(String),
            F32(ExternalFloat),
            F64(ExternalFloat),
            Char(char),
            String(String),
            Uuid(uuid::Uuid),
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
                inner: RequiredNullable<Box<ExternalSchemaValue>>,
            },
            Result(ExternalResultValuePayload),
            Text(ExternalTextValuePayload),
            Binary(ExternalBinaryValuePayload),
            Path {
                path: String,
            },
            Url {
                url: String,
            },
            Datetime {
                value: DateTime<Utc>,
            },
            Duration(ExternalDurationValuePayload),
            Quantity(ExternalQuantityValue),
            Union(ExternalUnionValuePayload),
        }

        #[derive(serde::Serialize, serde::Deserialize, golem_schema_derive::PoemSchema)]
        #[serde(rename_all = "camelCase")]
        pub struct ExternalVariantValuePayload {
            pub case: u32,
            #[serde(default, skip_serializing_if = "OptionalNonNull::is_none")]
            pub payload: OptionalNonNull<Box<ExternalSchemaValue>>,
        }

        #[derive(serde::Serialize, serde::Deserialize, golem_schema_derive::PoemSchema)]
        #[serde(tag = "tag", rename_all = "kebab-case")]
        pub enum ExternalResultValuePayload {
            Ok {
                value: RequiredNullable<Box<ExternalSchemaValue>>,
            },
            Err {
                value: RequiredNullable<Box<ExternalSchemaValue>>,
            },
        }

        #[derive(serde::Serialize, serde::Deserialize, golem_schema_derive::PoemSchema)]
        #[serde(rename_all = "camelCase")]
        pub struct ExternalTextValuePayload {
            pub text: String,
            #[serde(default, skip_serializing_if = "OptionalNonNull::is_none")]
            pub language: OptionalNonNull<String>,
        }

        #[derive(serde::Serialize, serde::Deserialize, golem_schema_derive::PoemSchema)]
        #[serde(rename_all = "camelCase")]
        pub struct ExternalBinaryValuePayload {
            pub bytes: Vec<u8>,
            #[serde(default, skip_serializing_if = "OptionalNonNull::is_none")]
            pub mime_type: OptionalNonNull<String>,
        }

        #[derive(serde::Serialize, serde::Deserialize, golem_schema_derive::PoemSchema)]
        #[serde(rename_all = "camelCase")]
        pub struct ExternalUnionValuePayload {
            pub tag: String,
            pub body: Box<ExternalSchemaValue>,
        }

        #[derive(serde::Serialize, serde::Deserialize, golem_schema_derive::PoemSchema)]
        #[serde(rename_all = "camelCase")]
        pub struct ExternalDurationValuePayload {
            pub nanoseconds: String,
        }

        #[derive(serde::Serialize, serde::Deserialize, golem_schema_derive::PoemSchema)]
        #[serde(rename_all = "camelCase")]
        pub struct ExternalQuantityValue {
            pub mantissa: String,
            pub scale: i32,
            pub unit: String,
        }

        #[derive(serde::Serialize, serde::Deserialize, golem_schema_derive::PoemSchema)]
        #[serde(rename_all = "camelCase")]
        pub struct ExternalTypedSchemaValue {
            pub graph: SchemaGraph,
            pub value: ExternalSchemaValue,
        }
    }

    macro_rules! impl_external_poem_type {
        ($external:ty, $schema:ty, $name:literal) => {
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
                    let value = value.ok_or_else(|| ParseError::expected_input())?;
                    serde_json::from_value(value).map_err(ParseError::custom)
                }
            }

            impl ToJSON for $external {
                fn to_json(&self) -> Option<Value> {
                    serde_json::to_value(self).ok()
                }
            }
        };
    }

    impl_external_poem_type!(
        ExternalSchemaValue,
        openapi_schema::ExternalSchemaValue,
        "ExternalSchemaValue"
    );
    impl_external_poem_type!(
        ExternalTypedSchemaValue,
        openapi_schema::ExternalTypedSchemaValue,
        "ExternalTypedSchemaValue"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{
        BinaryValuePayload, DurationValuePayload, QuantityValue, QuotaTokenValuePayload,
        ResultValuePayload, SchemaGraph, SchemaType, SchemaTypeDef, TextValuePayload,
        UnionValuePayload, VariantValuePayload,
    };
    use chrono::{TimeZone, Utc};
    use serde_json::{Value, json};
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

    fn assert_native_wire_round_trip(value: SchemaValue, expected: Value) {
        let external = ExternalSchemaValue::try_from(value.clone()).unwrap();
        let encoded = serde_json::to_value(&external).unwrap();
        assert_eq!(encoded, expected);

        let decoded: ExternalSchemaValue = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded.into_inner(), value);
    }

    #[test]
    fn native_external_value_wire_corpus_is_stable() {
        let datetime = Utc.timestamp_opt(1_725_190_496, 123_456_789).unwrap();
        let vectors = [
            (SchemaValue::Bool(true), json!({"kind":"bool","value":true})),
            (SchemaValue::S8(-8), json!({"kind":"s8","value":-8})),
            (SchemaValue::S16(-16), json!({"kind":"s16","value":-16})),
            (SchemaValue::S32(-32), json!({"kind":"s32","value":-32})),
            (
                SchemaValue::S64(i64::MIN),
                json!({"kind":"s64","value":i64::MIN.to_string()}),
            ),
            (SchemaValue::U8(8), json!({"kind":"u8","value":8})),
            (SchemaValue::U16(16), json!({"kind":"u16","value":16})),
            (SchemaValue::U32(32), json!({"kind":"u32","value":32})),
            (
                SchemaValue::U64(u64::MAX),
                json!({"kind":"u64","value":u64::MAX.to_string()}),
            ),
            (SchemaValue::F32(1.5), json!({"kind":"f32","value":1.5})),
            (SchemaValue::F32(0.1), json!({"kind":"f32","value":0.1})),
            (SchemaValue::F64(-0.0), json!({"kind":"f64","value":-0.0})),
            (SchemaValue::Char('λ'), json!({"kind":"char","value":"λ"})),
            (
                SchemaValue::String("Golem".to_string()),
                json!({"kind":"string","value":"Golem"}),
            ),
            (
                SchemaValue::Uuid("DD00721B-3329-4621-A01D-C71F02CD78C6".parse().unwrap()),
                json!({"kind":"uuid","value":"dd00721b-3329-4621-a01d-c71f02cd78c6"}),
            ),
            (
                SchemaValue::Record {
                    fields: vec![SchemaValue::U8(1), SchemaValue::String("x".to_string())],
                },
                json!({"kind":"record","value":{"fields":[
                    {"kind":"u8","value":1},
                    {"kind":"string","value":"x"}
                ]}}),
            ),
            (
                SchemaValue::Variant(VariantValuePayload {
                    case: 2,
                    payload: Some(Box::new(SchemaValue::String("payload".to_string()))),
                }),
                json!({"kind":"variant","value":{
                    "case":2,
                    "payload":{"kind":"string","value":"payload"}
                }}),
            ),
            (
                SchemaValue::Enum { case: 3 },
                json!({"kind":"enum","value":{"case":3}}),
            ),
            (
                SchemaValue::Flags {
                    bits: vec![true, false, true],
                },
                json!({"kind":"flags","value":{"bits":[true,false,true]}}),
            ),
            (
                SchemaValue::Tuple {
                    elements: vec![SchemaValue::Bool(true), SchemaValue::U8(2)],
                },
                json!({"kind":"tuple","value":{"elements":[
                    {"kind":"bool","value":true},
                    {"kind":"u8","value":2}
                ]}}),
            ),
            (
                SchemaValue::List {
                    elements: vec![SchemaValue::U8(1), SchemaValue::U8(2)],
                },
                json!({"kind":"list","value":{"elements":[
                    {"kind":"u8","value":1},
                    {"kind":"u8","value":2}
                ]}}),
            ),
            (
                SchemaValue::FixedList {
                    elements: vec![SchemaValue::U8(1), SchemaValue::U8(2)],
                },
                json!({"kind":"fixed-list","value":{"elements":[
                    {"kind":"u8","value":1},
                    {"kind":"u8","value":2}
                ]}}),
            ),
            (
                SchemaValue::Map {
                    entries: vec![(SchemaValue::String("key".to_string()), SchemaValue::U8(1))],
                },
                json!({"kind":"map","value":{"entries":[[
                    {"kind":"string","value":"key"},
                    {"kind":"u8","value":1}
                ]]}}),
            ),
            (
                SchemaValue::Option { inner: None },
                json!({"kind":"option","value":{"inner":null}}),
            ),
            (
                SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::Option { inner: None })),
                },
                json!({"kind":"option","value":{"inner":{
                    "kind":"option","value":{"inner":null}
                }}}),
            ),
            (
                SchemaValue::Result(ResultValuePayload::Ok { value: None }),
                json!({"kind":"result","value":{"tag":"ok","value":null}}),
            ),
            (
                SchemaValue::Result(ResultValuePayload::Err {
                    value: Some(Box::new(SchemaValue::String("failed".to_string()))),
                }),
                json!({"kind":"result","value":{
                    "tag":"err","value":{"kind":"string","value":"failed"}
                }}),
            ),
            (
                SchemaValue::Text(TextValuePayload {
                    text: "hello".to_string(),
                    language: Some("en-GB".to_string()),
                }),
                json!({"kind":"text","value":{"text":"hello","language":"en-GB"}}),
            ),
            (
                SchemaValue::Binary(BinaryValuePayload {
                    bytes: vec![251, 255],
                    mime_type: Some("application/octet-stream".to_string()),
                }),
                json!({"kind":"binary","value":{
                    "bytes":[251,255],"mimeType":"application/octet-stream"
                }}),
            ),
            (
                SchemaValue::Path {
                    path: "/tmp/input.wav".to_string(),
                },
                json!({"kind":"path","value":{"path":"/tmp/input.wav"}}),
            ),
            (
                SchemaValue::Url {
                    url: "https://example.com/a".to_string(),
                },
                json!({"kind":"url","value":{"url":"https://example.com/a"}}),
            ),
            (
                SchemaValue::Datetime { value: datetime },
                json!({"kind":"datetime","value":{"value":"2024-09-01T11:34:56.123456789Z"}}),
            ),
            (
                SchemaValue::Duration(DurationValuePayload {
                    nanoseconds: i64::MIN,
                }),
                json!({"kind":"duration","value":{"nanoseconds":i64::MIN.to_string()}}),
            ),
            (
                SchemaValue::Quantity(QuantityValue {
                    mantissa: 9_007_199_254_740_993,
                    scale: 3,
                    unit: "kg".to_string(),
                }),
                json!({"kind":"quantity","value":{
                    "mantissa":"9007199254740993","scale":3,"unit":"kg"
                }}),
            ),
            (
                SchemaValue::Union(UnionValuePayload {
                    tag: "number".to_string(),
                    body: Box::new(SchemaValue::String("number:7".to_string())),
                }),
                json!({"kind":"union","value":{
                    "tag":"number","body":{"kind":"string","value":"number:7"}
                }}),
            ),
        ];

        for (value, expected) in vectors {
            assert_native_wire_round_trip(value, expected);
        }
    }

    #[test]
    fn native_external_typed_value_carries_a_self_contained_graph() {
        let type_id = crate::schema::metadata::TypeId::new("Count");
        let graph = SchemaGraph {
            defs: vec![SchemaTypeDef {
                id: type_id.clone(),
                name: Some("Count".to_string()),
                body: SchemaType::u64(),
            }],
            root: SchemaType::ref_to(type_id),
        };
        let typed = TypedSchemaValue::new(graph.clone(), SchemaValue::U64(42));
        let external = ExternalTypedSchemaValue::try_from(typed.clone()).unwrap();
        let encoded = serde_json::to_value(external).unwrap();

        assert_eq!(encoded["graph"], serde_json::to_value(graph).unwrap());
        assert_eq!(encoded["value"], json!({"kind":"u64","value":"42"}));
        let decoded: ExternalTypedSchemaValue = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded.into_inner(), typed);
    }

    #[test]
    fn native_external_float_wire_preserves_exceptional_values_and_signed_zero() {
        let vectors = [
            (
                SchemaValue::F32(f32::NAN),
                json!({"kind":"f32","value":{"$float":"nan"}}),
            ),
            (
                SchemaValue::F64(f64::INFINITY),
                json!({"kind":"f64","value":{"$float":"positive-infinity"}}),
            ),
            (
                SchemaValue::F64(f64::NEG_INFINITY),
                json!({"kind":"f64","value":{"$float":"negative-infinity"}}),
            ),
        ];
        for (value, expected) in vectors {
            let external = ExternalSchemaValue::try_from(value).unwrap();
            let encoded = serde_json::to_value(external).unwrap();
            assert_eq!(encoded, expected);
            let decoded: ExternalSchemaValue = serde_json::from_value(encoded).unwrap();
            match decoded.into_inner() {
                SchemaValue::F32(value) => assert!(value.is_nan()),
                SchemaValue::F64(value) if value.is_infinite() => {}
                value => panic!("unexpected decoded value: {value:?}"),
            }
        }

        let encoded =
            serde_json::to_string(&ExternalSchemaValue::try_from(SchemaValue::F64(-0.0)).unwrap())
                .unwrap();
        assert_eq!(encoded, r#"{"kind":"f64","value":-0.0}"#);
        let decoded: ExternalSchemaValue = serde_json::from_str(&encoded).unwrap();
        let SchemaValue::F64(value) = decoded.into_inner() else {
            panic!("expected f64")
        };
        assert!(value.is_sign_negative());
    }

    #[test]
    fn native_external_value_rejects_noncanonical_scalars_and_unknown_fields() {
        for value in [
            json!({"kind":"u64","value":42}),
            json!({"kind":"u64","value":"042"}),
            json!({"kind":"u64","value":"18446744073709551616"}),
            json!({"kind":"s64","value":"-0"}),
            json!({"kind":"s64","value":"9223372036854775808"}),
            json!({"kind":"uuid","value":"DD00721B-3329-4621-A01D-C71F02CD78C6"}),
            json!({"kind":"uuid","value":"dd00721b33294621a01dc71f02cd78c6"}),
            json!({"kind":"uuid","value":"not-a-uuid"}),
            json!({"kind":"f64","value":{"$float":"infinity"}}),
            json!({"kind":"f32","value":3.5e38}),
            json!({"kind":"u8","value":1,"extra":true}),
            json!({"kind":"record","value":{"fields":[],"extra":true}}),
            json!({"kind":"record","value":{"fields":[
                {"kind":"u8","value":1,"extra":true}
            ]}}),
            json!({"kind":"binary","value":{"bytes":[],"mimeType":null}}),
            json!({"kind":"binary","value":{"bytes":[256]}}),
        ] {
            assert!(
                serde_json::from_value::<ExternalSchemaValue>(value).is_err(),
                "malformed external value was accepted"
            );
        }
    }

    #[test]
    fn native_external_numeric_boundaries_round_trip_through_json_text() {
        for value in [
            SchemaValue::S64(i64::MIN),
            SchemaValue::S64(-9_007_199_254_740_993),
            SchemaValue::S64(9_007_199_254_740_993),
            SchemaValue::S64(i64::MAX),
            SchemaValue::U64(9_007_199_254_740_991),
            SchemaValue::U64(9_007_199_254_740_992),
            SchemaValue::U64(9_007_199_254_740_993),
            SchemaValue::U64(u64::MAX),
        ] {
            let encoded =
                serde_json::to_string(&ExternalSchemaValue::try_from(value.clone()).unwrap())
                    .unwrap();
            let decoded: ExternalSchemaValue = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded.into_inner(), value);
        }

        let nested = SchemaValue::Map {
            entries: vec![(
                SchemaValue::S64(-9_007_199_254_740_993),
                SchemaValue::List {
                    elements: vec![
                        SchemaValue::U64(9_007_199_254_740_993),
                        SchemaValue::Duration(DurationValuePayload {
                            nanoseconds: i64::MIN,
                        }),
                        SchemaValue::Quantity(QuantityValue {
                            mantissa: i64::MAX,
                            scale: -4,
                            unit: "m".to_string(),
                        }),
                    ],
                },
            )],
        };
        let encoded =
            serde_json::to_string(&ExternalSchemaValue::try_from(nested.clone()).unwrap()).unwrap();
        assert!(encoded.contains(r#""9007199254740993""#));
        let decoded: ExternalSchemaValue = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.into_inner(), nested);
    }

    #[test]
    fn native_external_float_widths_round_trip_without_semantic_loss() {
        for value in [
            0.0f32,
            -0.0,
            f32::MIN_POSITIVE,
            f32::from_bits(1),
            f32::MAX,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NAN,
        ] {
            let encoded = serde_json::to_string(
                &ExternalSchemaValue::try_from(SchemaValue::F32(value)).unwrap(),
            )
            .unwrap();
            let decoded: ExternalSchemaValue = serde_json::from_str(&encoded).unwrap();
            let SchemaValue::F32(decoded) = decoded.into_inner() else {
                panic!("expected f32")
            };
            if value.is_nan() {
                assert!(decoded.is_nan());
            } else {
                assert_eq!(decoded.to_bits(), value.to_bits());
            }
        }

        for value in [
            0.0f64,
            -0.0,
            f64::MIN_POSITIVE,
            f64::from_bits(1),
            f64::MAX,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
        ] {
            let encoded = serde_json::to_string(
                &ExternalSchemaValue::try_from(SchemaValue::F64(value)).unwrap(),
            )
            .unwrap();
            let decoded: ExternalSchemaValue = serde_json::from_str(&encoded).unwrap();
            let SchemaValue::F64(decoded) = decoded.into_inner() else {
                panic!("expected f64")
            };
            if value.is_nan() {
                assert!(decoded.is_nan());
            } else {
                assert_eq!(decoded.to_bits(), value.to_bits());
            }
        }
    }

    #[test]
    fn native_external_optional_payload_rules_are_strict() {
        let accepted = [
            json!({"kind":"variant","value":{"case":0}}),
            json!({"kind":"text","value":{"text":"hello"}}),
            json!({"kind":"binary","value":{"bytes":[]}}),
            json!({"kind":"option","value":{"inner":null}}),
            json!({"kind":"result","value":{"tag":"ok","value":null}}),
            json!({"kind":"result","value":{"tag":"err","value":null}}),
        ];
        for value in accepted {
            serde_json::from_value::<ExternalSchemaValue>(value).unwrap();
        }

        let rejected = [
            json!({"kind":"variant","value":{"case":0,"payload":null}}),
            json!({"kind":"text","value":{"text":"hello","language":null}}),
            json!({"kind":"option","value":{}}),
            json!({"kind":"result","value":{"tag":"ok"}}),
        ];
        for value in rejected {
            assert!(serde_json::from_value::<ExternalSchemaValue>(value).is_err());
        }
    }

    #[cfg(feature = "full")]
    #[test]
    fn poem_external_value_codec_matches_serde_codec() {
        use poem_openapi::types::{ParseFromJSON, ToJSON};

        let value = ExternalSchemaValue::try_from(SchemaValue::Record {
            fields: vec![
                SchemaValue::U64(u64::MAX),
                SchemaValue::F64(f64::NEG_INFINITY),
            ],
        })
        .unwrap();
        let serde_json = serde_json::to_value(&value).unwrap();
        assert_eq!(value.to_json().unwrap(), serde_json);
        let decoded = ExternalSchemaValue::parse_from_json(Some(serde_json)).unwrap();
        assert_eq!(decoded.into_inner(), value.into_inner());
    }

    #[test]
    fn native_external_typed_envelope_rejects_unknown_fields_and_capabilities() {
        let graph = SchemaGraph::anonymous(SchemaType::string());
        let safe_value = json!({"kind":"string","value":"safe"});
        assert!(
            serde_json::from_value::<ExternalTypedSchemaValue>(json!({
                "graph": graph,
                "value": safe_value,
                "extra": true
            }))
            .is_err()
        );

        let forged = TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::quota_token(Default::default())),
            forged_quota_token(),
        );
        let forged_json = serde_json::to_value(forged).unwrap();
        assert!(serde_json::from_value::<ExternalTypedSchemaValue>(forged_json).is_err());
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
        let value_schema_json = serde_json::to_value(
            registry
                .schemas
                .get("ExternalSchemaValue")
                .expect("external value schema must be registered"),
        )
        .unwrap();
        let value_schema = serde_json::to_string(&value_schema_json).unwrap();
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
        assert!(value_schema.contains("$float"));
        assert!(value_schema.contains("positive-infinity"));

        let variant_for = |kind: &str| {
            value_schema_json["oneOf"]
                .as_array()
                .unwrap()
                .iter()
                .find(|variant| variant["properties"]["kind"]["enum"] == json!([kind]))
                .unwrap()
        };
        for kind in ["s64", "u64"] {
            assert_eq!(variant_for(kind)["properties"]["value"]["type"], "string");
        }
        for kind in ["f32", "f64"] {
            let float = &variant_for(kind)["properties"]["value"];
            assert_eq!(float["oneOf"][0]["type"], "number");
            assert_eq!(float["oneOf"][1]["properties"]["$float"]["type"], "string");
        }
        let option = &variant_for("option")["properties"]["value"];
        assert!(
            option["required"]
                .as_array()
                .unwrap()
                .contains(&json!("inner"))
        );
        assert_eq!(option["properties"]["inner"]["oneOf"][1]["nullable"], true);
        assert_eq!(
            option["properties"]["inner"]["oneOf"][1]["enum"],
            json!([null])
        );

        let duration = serde_json::to_value(
            registry
                .schemas
                .get("ExternalDurationValuePayload")
                .expect("external duration schema must be registered"),
        )
        .unwrap();
        assert_eq!(duration["properties"]["nanoseconds"]["type"], "string");

        let quantity = serde_json::to_value(
            registry
                .schemas
                .get("ExternalQuantityValue")
                .expect("external quantity schema must be registered"),
        )
        .unwrap();
        assert_eq!(quantity["properties"]["mantissa"]["type"], "string");

        let text = serde_json::to_value(
            registry
                .schemas
                .get("ExternalTextValuePayload")
                .expect("external text schema must be registered"),
        )
        .unwrap();
        assert_eq!(text["required"], json!(["text"]));
        assert_ne!(text["properties"]["language"]["nullable"], true);

        let binary = serde_json::to_value(
            registry
                .schemas
                .get("ExternalBinaryValuePayload")
                .expect("external binary schema must be registered"),
        )
        .unwrap();
        assert_eq!(binary["required"], json!(["bytes"]));
        assert_ne!(binary["properties"]["mimeType"]["nullable"], true);

        let variant = serde_json::to_value(
            registry
                .schemas
                .get("ExternalVariantValuePayload")
                .expect("external variant schema must be registered"),
        )
        .unwrap();
        assert_eq!(variant["required"], json!(["case"]));
        assert_ne!(variant["properties"]["payload"]["nullable"], true);

        let result = serde_json::to_value(
            registry
                .schemas
                .get("ExternalResultValuePayload")
                .expect("external result schema must be registered"),
        )
        .unwrap();
        for branch in result["oneOf"].as_array().unwrap() {
            assert!(
                branch["required"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("value"))
            );
            assert_eq!(branch["properties"]["value"]["oneOf"][1]["nullable"], true);
            assert_eq!(
                branch["properties"]["value"]["oneOf"][1]["enum"],
                json!([null])
            );
        }
    }
}
