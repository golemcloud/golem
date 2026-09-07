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

use crate::model::invocation_session_public::{
    MAX_COLLECTION_SIZE, MAX_JSON_DEPTH, MAX_LOGICAL_VALUE_SIZE, MAX_TOKEN_SIZE, PublicErrorCode,
};
use crate::schema::render::{from_json_value, to_json_value};
use crate::schema::stream::SchemaValueStream;
use crate::schema::validation::value::validate_value;
use crate::schema::{
    BinaryValuePayload, DurationValuePayload, ResultValuePayload, SchemaGraph, SchemaType,
    SchemaValue, UnionValuePayload, VariantValuePayload,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Number, Value};
use std::collections::HashSet;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublicStreamReference {
    Provisional(Uuid),
    Stable(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicStreamReferencePolicy {
    None,
    Provisional,
    Stable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicSchemaValueError {
    pub code: PublicErrorCode,
    pub message: String,
}

impl PublicSchemaValueError {
    pub fn new(code: PublicErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn malformed(message: impl Into<String>) -> Self {
        Self::new(PublicErrorCode::MalformedMessage, message)
    }

    fn validation(message: impl Into<String>) -> Self {
        Self::new(PublicErrorCode::ValidationError, message)
    }
}

impl Display for PublicSchemaValueError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for PublicSchemaValueError {}

pub fn decode_public_schema_value(
    graph: &SchemaGraph,
    ty: &SchemaType,
    json: &Value,
    stream_policy: PublicStreamReferencePolicy,
    mut resolve_stream: impl FnMut(
        PublicStreamReference,
        Option<&SchemaType>,
    ) -> Result<SchemaValueStream, PublicSchemaValueError>,
) -> Result<SchemaValue, PublicSchemaValueError> {
    decode_public_schema_value_with_charge(graph, ty, json, stream_policy, &mut resolve_stream)
        .map(|(value, _)| value)
}

pub fn decode_public_schema_value_with_charge(
    graph: &SchemaGraph,
    ty: &SchemaType,
    json: &Value,
    stream_policy: PublicStreamReferencePolicy,
    mut resolve_stream: impl FnMut(
        PublicStreamReference,
        Option<&SchemaType>,
    ) -> Result<SchemaValueStream, PublicSchemaValueError>,
) -> Result<(SchemaValue, usize), PublicSchemaValueError> {
    let mut decoder = Decoder {
        graph,
        stream_policy,
        resolve_stream: &mut resolve_stream,
        provisional_refs: HashSet::new(),
        depth: 0,
        charge: 0,
    };
    let value = decoder.decode(ty, json)?;
    validate_value(graph, ty, &value).map_err(|_| {
        PublicSchemaValueError::validation("value does not satisfy the selected schema")
    })?;
    Ok((value, decoder.charge as usize))
}

pub fn encode_public_schema_value(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
    mut resolve_stream: impl FnMut(
        &SchemaValueStream,
        Option<&SchemaType>,
    ) -> Result<PublicStreamReference, PublicSchemaValueError>,
) -> Result<Value, PublicSchemaValueError> {
    encode_public_schema_value_with_charge(graph, ty, value, &mut resolve_stream)
        .map(|(value, _)| value)
}

pub fn encode_public_schema_value_with_charge(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
    mut resolve_stream: impl FnMut(
        &SchemaValueStream,
        Option<&SchemaType>,
    ) -> Result<PublicStreamReference, PublicSchemaValueError>,
) -> Result<(Value, usize), PublicSchemaValueError> {
    validate_value(graph, ty, value).map_err(|_| {
        PublicSchemaValueError::validation("value does not satisfy the selected schema")
    })?;
    let mut encoder = Encoder {
        graph,
        resolve_stream: &mut resolve_stream,
        depth: 0,
        charge: 0,
    };
    let value = encoder.encode(ty, value)?;
    Ok((value, encoder.charge as usize))
}

struct Decoder<'a, F> {
    graph: &'a SchemaGraph,
    stream_policy: PublicStreamReferencePolicy,
    resolve_stream: &'a mut F,
    provisional_refs: HashSet<Uuid>,
    depth: usize,
    charge: u64,
}

impl<F> Decoder<'_, F>
where
    F: FnMut(
        PublicStreamReference,
        Option<&SchemaType>,
    ) -> Result<SchemaValueStream, PublicSchemaValueError>,
{
    fn decode(
        &mut self,
        ty: &SchemaType,
        json: &Value,
    ) -> Result<SchemaValue, PublicSchemaValueError> {
        self.depth += 1;
        if self.depth > MAX_JSON_DEPTH {
            return Err(resource_error("schema value nesting exceeds 64 levels"));
        }
        self.charge(1)?;
        let ty = resolve_type(self.graph, ty)?;
        let result = match ty {
            SchemaType::S64 { .. } => {
                self.charge(8)?;
                parse_s64(json).map(SchemaValue::S64)
            }
            SchemaType::U64 { .. } => {
                self.charge(8)?;
                parse_u64(json).map(SchemaValue::U64)
            }
            SchemaType::F32 { .. } => parse_float(json).and_then(|value| {
                self.charge(4)?;
                let narrowed = value as f32;
                if narrowed.is_infinite() && value.is_finite() {
                    Err(PublicSchemaValueError::validation("f32 is out of range"))
                } else {
                    Ok(SchemaValue::F32(narrowed))
                }
            }),
            SchemaType::F64 { .. } => {
                self.charge(8)?;
                parse_float(json).map(SchemaValue::F64)
            }
            SchemaType::Binary { .. } => self.binary(json),
            SchemaType::Duration { .. } => {
                self.charge(8)?;
                let object = exact_object(json, &["nanoseconds"])?;
                parse_s64(object.get("nanoseconds").unwrap())
                    .map(|nanoseconds| SchemaValue::Duration(DurationValuePayload { nanoseconds }))
            }
            SchemaType::Quantity { .. } => {
                let object = exact_object(json, &["mantissa", "scale", "unit"])?;
                let mantissa = parse_s64(object.get("mantissa").unwrap())?;
                let scale: i32 = object
                    .get("scale")
                    .and_then(Value::as_i64)
                    .and_then(|value| value.try_into().ok())
                    .ok_or_else(|| PublicSchemaValueError::validation("invalid quantity scale"))?;
                let unit = required_string(object, "unit")?.to_string();
                self.charge(12 + unit.len() as u64)?;
                Ok(SchemaValue::Quantity(crate::schema::QuantityValue {
                    mantissa,
                    scale,
                    unit,
                }))
            }
            SchemaType::Record { fields, .. } => {
                let object = json.as_object().ok_or_else(|| {
                    PublicSchemaValueError::validation("record must be a JSON object")
                })?;
                self.collection(object.len())?;
                if object.len() != fields.len()
                    || object
                        .keys()
                        .any(|name| !fields.iter().any(|field| field.name == *name))
                {
                    return Err(PublicSchemaValueError::validation(
                        "record members must exactly match the schema",
                    ));
                }
                let mut values = Vec::with_capacity(fields.len());
                for field in fields {
                    let json = object.get(&field.name).ok_or_else(|| {
                        PublicSchemaValueError::validation("record field is missing")
                    })?;
                    self.charge(field.name.len() as u64)?;
                    values.push(self.decode(&field.body, json)?);
                }
                Ok(SchemaValue::Record { fields: values })
            }
            SchemaType::Variant { cases, .. } => {
                let object = tagged_object(json, "$case")?;
                let name = required_string(object, "$case")?;
                let (index, case) = cases
                    .iter()
                    .enumerate()
                    .find(|(_, case)| case.name == name)
                    .ok_or_else(|| PublicSchemaValueError::validation("unknown variant case"))?;
                self.charge(name.len() as u64)?;
                let payload = match (&case.payload, object.get("value")) {
                    (None, None) => None,
                    (Some(ty), Some(value)) => Some(Box::new(self.decode(ty, value)?)),
                    _ => {
                        return Err(PublicSchemaValueError::validation(
                            "variant payload presence does not match the schema",
                        ));
                    }
                };
                Ok(SchemaValue::Variant(VariantValuePayload {
                    case: index as u32,
                    payload,
                }))
            }
            SchemaType::Enum { cases, .. } => {
                let name = json.as_str().ok_or_else(|| {
                    PublicSchemaValueError::validation("enum must be a JSON string")
                })?;
                let case = cases
                    .iter()
                    .position(|case| case == name)
                    .ok_or_else(|| PublicSchemaValueError::validation("unknown enum case"))?;
                self.charge(name.len() as u64)?;
                Ok(SchemaValue::Enum { case: case as u32 })
            }
            SchemaType::Flags { flags, .. } => {
                let values = json.as_array().ok_or_else(|| {
                    PublicSchemaValueError::validation("flags must be a JSON array")
                })?;
                self.collection(values.len())?;
                let mut bits = vec![false; flags.len()];
                let mut previous = None;
                for value in values {
                    let name = value.as_str().ok_or_else(|| {
                        PublicSchemaValueError::validation("flag name must be a string")
                    })?;
                    let index = flags
                        .iter()
                        .position(|flag| flag == name)
                        .ok_or_else(|| PublicSchemaValueError::validation("unknown flag"))?;
                    self.charge(name.len() as u64)?;
                    if bits[index] || previous.is_some_and(|previous| index <= previous) {
                        return Err(PublicSchemaValueError::validation(
                            "flags must be unique and in declaration order",
                        ));
                    }
                    bits[index] = true;
                    previous = Some(index);
                }
                Ok(SchemaValue::Flags { bits })
            }
            SchemaType::Tuple { elements, .. } => {
                let values = exact_array(json, elements.len(), "tuple")?;
                self.collection(values.len())?;
                let mut decoded = Vec::with_capacity(values.len());
                for (ty, value) in elements.iter().zip(values) {
                    decoded.push(self.decode(ty, value)?);
                }
                Ok(SchemaValue::Tuple { elements: decoded })
            }
            SchemaType::List { element, .. } => {
                let values = array(json, "list")?;
                self.collection(values.len())?;
                let mut decoded = Vec::with_capacity(values.len());
                for value in values {
                    decoded.push(self.decode(element, value)?);
                }
                Ok(SchemaValue::List { elements: decoded })
            }
            SchemaType::FixedList {
                element, length, ..
            } => {
                let values = exact_array(json, *length as usize, "fixed list")?;
                self.collection(values.len())?;
                let mut decoded = Vec::with_capacity(values.len());
                for value in values {
                    decoded.push(self.decode(element, value)?);
                }
                Ok(SchemaValue::FixedList { elements: decoded })
            }
            SchemaType::Map { key, value, .. } => {
                let entries = array(json, "map")?;
                self.collection(entries.len())?;
                let mut decoded = Vec::with_capacity(entries.len());
                for entry in entries {
                    let pair = exact_array(entry, 2, "map entry")?;
                    decoded.push((self.decode(key, &pair[0])?, self.decode(value, &pair[1])?));
                }
                Ok(SchemaValue::Map { entries: decoded })
            }
            SchemaType::Option { inner, .. } => {
                let object = tagged_object(json, "$option")?;
                let discriminator = required_string(object, "$option")?;
                self.charge(discriminator.len() as u64)?;
                match discriminator {
                    "none" if !object.contains_key("value") => {
                        Ok(SchemaValue::Option { inner: None })
                    }
                    "some" => {
                        let value = object.get("value").ok_or_else(|| {
                            PublicSchemaValueError::validation("some option requires a value")
                        })?;
                        Ok(SchemaValue::Option {
                            inner: Some(Box::new(self.decode(inner, value)?)),
                        })
                    }
                    _ => Err(PublicSchemaValueError::validation(
                        "invalid option representation",
                    )),
                }
            }
            SchemaType::Result { spec, .. } => {
                let object = tagged_object(json, "$result")?;
                let arm = required_string(object, "$result")?;
                self.charge(arm.len() as u64)?;
                let (ty, ok) = match arm {
                    "ok" => (spec.ok.as_deref(), true),
                    "err" => (spec.err.as_deref(), false),
                    _ => {
                        return Err(PublicSchemaValueError::validation(
                            "invalid result discriminator",
                        ));
                    }
                };
                let payload = match (ty, object.get("value")) {
                    (None, None) => None,
                    (Some(ty), Some(value)) => Some(Box::new(self.decode(ty, value)?)),
                    _ => {
                        return Err(PublicSchemaValueError::validation(
                            "result payload presence does not match the schema",
                        ));
                    }
                };
                if ok {
                    Ok(SchemaValue::Result(ResultValuePayload::Ok {
                        value: payload,
                    }))
                } else {
                    Ok(SchemaValue::Result(ResultValuePayload::Err {
                        value: payload,
                    }))
                }
            }
            SchemaType::Union { spec, .. } => {
                let object = tagged_object(json, "$union")?;
                let tag = required_string(object, "$union")?;
                self.charge(tag.len() as u64)?;
                let branch = spec
                    .branches
                    .iter()
                    .find(|branch| branch.tag == tag)
                    .ok_or_else(|| PublicSchemaValueError::validation("unknown union branch"))?;
                let value = object
                    .get("value")
                    .ok_or_else(|| PublicSchemaValueError::validation("union requires a value"))?;
                Ok(SchemaValue::Union(UnionValuePayload {
                    tag: tag.to_string(),
                    body: Box::new(self.decode(&branch.body, value)?),
                }))
            }
            SchemaType::Stream { inner, .. } => self.stream(json, inner.as_deref()),
            SchemaType::Secret { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::PermissionCard { .. }
            | SchemaType::Future { .. } => Err(PublicSchemaValueError::new(
                PublicErrorCode::UnsupportedValue,
                "host-managed capabilities and futures cannot cross the public boundary",
            )),
            _ => {
                let value = from_json_value(self.graph, ty, json).map_err(|_| {
                    PublicSchemaValueError::validation("value does not match the selected schema")
                })?;
                self.charge(scalar_payload_charge(&value, json)?)?;
                Ok(value)
            }
        };
        self.depth -= 1;
        result
    }

    fn binary(&mut self, json: &Value) -> Result<SchemaValue, PublicSchemaValueError> {
        let object = json.as_object().ok_or_else(|| {
            PublicSchemaValueError::validation("binary value must be a JSON object")
        })?;
        if object.is_empty()
            || object.len() > 2
            || object.keys().any(|key| key != "bytes" && key != "mimeType")
        {
            return Err(PublicSchemaValueError::malformed(
                "binary value contains invalid members",
            ));
        }
        let encoded = required_string(object, "bytes")?;
        let bytes = STANDARD.decode(encoded).map_err(|_| {
            PublicSchemaValueError::malformed("binary bytes are not canonical padded base64")
        })?;
        if STANDARD.encode(&bytes) != encoded {
            return Err(PublicSchemaValueError::malformed(
                "binary bytes are not canonical padded base64",
            ));
        }
        let mime_type = object
            .get("mimeType")
            .map(|value| {
                value
                    .as_str()
                    .filter(|value| valid_mime_type(value))
                    .map(str::to_string)
                    .ok_or_else(|| PublicSchemaValueError::validation("invalid binary MIME type"))
            })
            .transpose()?;
        self.charge(
            bytes.len() as u64
                + mime_type
                    .as_ref()
                    .map(|value| value.len() as u64)
                    .unwrap_or_default(),
        )?;
        Ok(SchemaValue::Binary(BinaryValuePayload { bytes, mime_type }))
    }

    fn stream(
        &mut self,
        json: &Value,
        element: Option<&SchemaType>,
    ) -> Result<SchemaValue, PublicSchemaValueError> {
        let outer = exact_object(json, &["$stream"])?;
        let inner = outer["$stream"]
            .as_object()
            .ok_or_else(|| PublicSchemaValueError::validation("$stream must be an object"))?;
        if inner.len() != 1 {
            return Err(PublicSchemaValueError::validation(
                "stream reference must have exactly one identity",
            ));
        }
        let reference = match self.stream_policy {
            PublicStreamReferencePolicy::Provisional => {
                let encoded = required_string(inner, "provisionalRef")?;
                let reference = encoded.parse::<Uuid>().map_err(|_| {
                    PublicSchemaValueError::validation("invalid provisional stream UUID")
                })?;
                if reference.get_variant() != uuid::Variant::RFC4122
                    || reference.get_version_num() != 4
                    || reference.to_string() != encoded
                {
                    return Err(PublicSchemaValueError::validation(
                        "provisional stream reference must be canonical UUIDv4",
                    ));
                }
                if !self.provisional_refs.insert(reference) {
                    return Err(PublicSchemaValueError::new(
                        PublicErrorCode::StreamAlreadyConsumed,
                        "provisional stream reference appears more than once",
                    ));
                }
                self.charge(16)?;
                PublicStreamReference::Provisional(reference)
            }
            PublicStreamReferencePolicy::Stable => {
                let token = required_string(inner, "streamToken")?;
                if token.is_empty() || token.len() > MAX_TOKEN_SIZE {
                    return Err(PublicSchemaValueError::new(
                        PublicErrorCode::TokenInvalid,
                        "invalid stream token length",
                    ));
                }
                self.charge(token.len() as u64)?;
                PublicStreamReference::Stable(token.to_string())
            }
            PublicStreamReferencePolicy::None => {
                return Err(PublicSchemaValueError::new(
                    PublicErrorCode::UnsupportedValue,
                    "stream references are not allowed in this value",
                ));
            }
        };
        (self.resolve_stream)(reference, element).map(SchemaValue::Stream)
    }

    fn collection(&mut self, len: usize) -> Result<(), PublicSchemaValueError> {
        if len > MAX_COLLECTION_SIZE {
            return Err(resource_error("collection exceeds 100000 entries"));
        }
        self.charge(4)
    }

    fn charge(&mut self, bytes: u64) -> Result<(), PublicSchemaValueError> {
        self.charge = self
            .charge
            .checked_add(bytes)
            .ok_or_else(|| resource_error("schema value byte charge overflows"))?;
        if self.charge > MAX_LOGICAL_VALUE_SIZE as u64 {
            return Err(resource_error("decoded schema value exceeds 16 MiB"));
        }
        Ok(())
    }
}

struct Encoder<'a, F> {
    graph: &'a SchemaGraph,
    resolve_stream: &'a mut F,
    depth: usize,
    charge: u64,
}

impl<F> Encoder<'_, F>
where
    F: FnMut(
        &SchemaValueStream,
        Option<&SchemaType>,
    ) -> Result<PublicStreamReference, PublicSchemaValueError>,
{
    fn encode(
        &mut self,
        ty: &SchemaType,
        value: &SchemaValue,
    ) -> Result<Value, PublicSchemaValueError> {
        self.depth += 1;
        if self.depth > MAX_JSON_DEPTH {
            return Err(resource_error("schema value nesting exceeds 64 levels"));
        }
        self.charge(1)?;
        let ty = resolve_type(self.graph, ty)?;
        let result = match (ty, value) {
            (SchemaType::S64 { .. }, SchemaValue::S64(value)) => {
                self.charge(8)?;
                Ok(Value::String(value.to_string()))
            }
            (SchemaType::U64 { .. }, SchemaValue::U64(value)) => {
                self.charge(8)?;
                Ok(Value::String(value.to_string()))
            }
            (SchemaType::F32 { .. }, SchemaValue::F32(value)) => {
                self.charge(4)?;
                encode_f32(*value)
            }
            (SchemaType::F64 { .. }, SchemaValue::F64(value)) => {
                self.charge(8)?;
                encode_float(*value)
            }
            (SchemaType::Binary { .. }, SchemaValue::Binary(value)) => {
                self.charge(
                    value.bytes.len() as u64
                        + value
                            .mime_type
                            .as_ref()
                            .map(|value| value.len() as u64)
                            .unwrap_or_default(),
                )?;
                let mut object = Map::new();
                object.insert(
                    "bytes".to_string(),
                    Value::String(STANDARD.encode(&value.bytes)),
                );
                if let Some(mime_type) = &value.mime_type {
                    object.insert("mimeType".to_string(), Value::String(mime_type.clone()));
                }
                Ok(Value::Object(object))
            }
            (SchemaType::Duration { .. }, SchemaValue::Duration(value)) => {
                self.charge(8)?;
                Ok(tagged_number_string(
                    "nanoseconds",
                    value.nanoseconds.to_string(),
                ))
            }
            (SchemaType::Quantity { .. }, SchemaValue::Quantity(value)) => {
                self.charge(12 + value.unit.len() as u64)?;
                let mut object = Map::new();
                object.insert(
                    "mantissa".to_string(),
                    Value::String(value.mantissa.to_string()),
                );
                object.insert("scale".to_string(), Value::Number(value.scale.into()));
                object.insert("unit".to_string(), Value::String(value.unit.clone()));
                Ok(Value::Object(object))
            }
            (SchemaType::Record { fields, .. }, SchemaValue::Record { fields: values }) => {
                self.collection(fields.len())?;
                let mut object = Map::new();
                for (field, value) in fields.iter().zip(values) {
                    self.charge(field.name.len() as u64)?;
                    object.insert(field.name.clone(), self.encode(&field.body, value)?);
                }
                Ok(Value::Object(object))
            }
            (SchemaType::Variant { cases, .. }, SchemaValue::Variant(value)) => {
                let case = cases.get(value.case as usize).ok_or_else(|| {
                    PublicSchemaValueError::validation("variant case is out of range")
                })?;
                self.charge(case.name.len() as u64)?;
                let mut object = Map::new();
                object.insert("$case".to_string(), Value::String(case.name.clone()));
                if let (Some(ty), Some(payload)) = (&case.payload, &value.payload) {
                    object.insert("value".to_string(), self.encode(ty, payload)?);
                }
                Ok(Value::Object(object))
            }
            (SchemaType::Enum { cases, .. }, SchemaValue::Enum { case }) => {
                let name = cases.get(*case as usize).ok_or_else(|| {
                    PublicSchemaValueError::validation("enum case is out of range")
                })?;
                self.charge(name.len() as u64)?;
                Ok(Value::String(name.clone()))
            }
            (SchemaType::Flags { flags, .. }, SchemaValue::Flags { bits }) => {
                let selected = flags
                    .iter()
                    .zip(bits)
                    .filter(|(_, selected)| **selected)
                    .map(|(flag, _)| flag)
                    .collect::<Vec<_>>();
                self.collection(selected.len())?;
                for flag in &selected {
                    self.charge(flag.len() as u64)?;
                }
                Ok(Value::Array(
                    selected
                        .into_iter()
                        .map(|flag| Value::String(flag.clone()))
                        .collect(),
                ))
            }
            (SchemaType::Tuple { elements, .. }, SchemaValue::Tuple { elements: values }) => {
                self.collection(elements.len())?;
                self.encode_list(elements.iter().zip(values))
            }
            (SchemaType::List { element, .. }, SchemaValue::List { elements })
            | (SchemaType::FixedList { element, .. }, SchemaValue::FixedList { elements }) => {
                self.collection(elements.len())?;
                self.encode_list(elements.iter().map(|value| (element.as_ref(), value)))
            }
            (SchemaType::Map { key, value: ty, .. }, SchemaValue::Map { entries }) => {
                self.collection(entries.len())?;
                let mut encoded = Vec::with_capacity(entries.len());
                for (key_value, value) in entries {
                    encoded.push(Value::Array(vec![
                        self.encode(key, key_value)?,
                        self.encode(ty, value)?,
                    ]));
                }
                Ok(Value::Array(encoded))
            }
            (SchemaType::Option { inner, .. }, SchemaValue::Option { inner: value }) => {
                let mut object = Map::new();
                match value {
                    None => {
                        self.charge(4)?;
                        object.insert("$option".to_string(), Value::String("none".to_string()));
                    }
                    Some(value) => {
                        self.charge(4)?;
                        object.insert("$option".to_string(), Value::String("some".to_string()));
                        object.insert("value".to_string(), self.encode(inner, value)?);
                    }
                }
                Ok(Value::Object(object))
            }
            (SchemaType::Result { spec, .. }, SchemaValue::Result(value)) => {
                let mut object = Map::new();
                match value {
                    ResultValuePayload::Ok { value } => {
                        self.charge(2)?;
                        object.insert("$result".to_string(), Value::String("ok".to_string()));
                        if let (Some(ty), Some(value)) = (&spec.ok, value) {
                            object.insert("value".to_string(), self.encode(ty, value)?);
                        }
                    }
                    ResultValuePayload::Err { value } => {
                        self.charge(3)?;
                        object.insert("$result".to_string(), Value::String("err".to_string()));
                        if let (Some(ty), Some(value)) = (&spec.err, value) {
                            object.insert("value".to_string(), self.encode(ty, value)?);
                        }
                    }
                }
                Ok(Value::Object(object))
            }
            (SchemaType::Union { spec, .. }, SchemaValue::Union(value)) => {
                let branch = spec
                    .branches
                    .iter()
                    .find(|branch| branch.tag == value.tag)
                    .ok_or_else(|| PublicSchemaValueError::validation("unknown union branch"))?;
                self.charge(value.tag.len() as u64)?;
                let mut object = Map::new();
                object.insert("$union".to_string(), Value::String(value.tag.clone()));
                object.insert("value".to_string(), self.encode(&branch.body, &value.body)?);
                Ok(Value::Object(object))
            }
            (SchemaType::Stream { inner, .. }, SchemaValue::Stream(value)) => {
                let reference = (self.resolve_stream)(value, inner.as_deref())?;
                let mut inner = Map::new();
                match reference {
                    PublicStreamReference::Provisional(reference) => {
                        self.charge(16)?;
                        inner.insert(
                            "provisionalRef".to_string(),
                            Value::String(reference.to_string()),
                        );
                    }
                    PublicStreamReference::Stable(token) => {
                        if token.is_empty() || token.len() > MAX_TOKEN_SIZE {
                            return Err(PublicSchemaValueError::new(
                                PublicErrorCode::TokenInvalid,
                                "invalid stream token length",
                            ));
                        }
                        self.charge(token.len() as u64)?;
                        inner.insert("streamToken".to_string(), Value::String(token));
                    }
                }
                let mut outer = Map::new();
                outer.insert("$stream".to_string(), Value::Object(inner));
                Ok(Value::Object(outer))
            }
            (SchemaType::Secret { .. }, _)
            | (SchemaType::QuotaToken { .. }, _)
            | (SchemaType::PermissionCard { .. }, _)
            | (SchemaType::Future { .. }, _) => Err(PublicSchemaValueError::new(
                PublicErrorCode::UnsupportedValue,
                "host-managed capabilities and futures cannot cross the public boundary",
            )),
            _ => {
                let json = to_json_value(self.graph, ty, value).map_err(|_| {
                    PublicSchemaValueError::validation("value does not match the selected schema")
                })?;
                self.charge(scalar_payload_charge(value, &json)?)?;
                Ok(json)
            }
        };
        self.depth -= 1;
        result
    }

    fn encode_list<'a>(
        &mut self,
        values: impl Iterator<Item = (&'a SchemaType, &'a SchemaValue)>,
    ) -> Result<Value, PublicSchemaValueError> {
        let mut encoded = Vec::new();
        for (ty, value) in values {
            encoded.push(self.encode(ty, value)?);
        }
        Ok(Value::Array(encoded))
    }

    fn collection(&mut self, len: usize) -> Result<(), PublicSchemaValueError> {
        if len > MAX_COLLECTION_SIZE {
            return Err(resource_error("collection exceeds 100000 entries"));
        }
        self.charge(4)
    }

    fn charge(&mut self, bytes: u64) -> Result<(), PublicSchemaValueError> {
        self.charge = self
            .charge
            .checked_add(bytes)
            .ok_or_else(|| resource_error("schema value byte charge overflows"))?;
        if self.charge > MAX_LOGICAL_VALUE_SIZE as u64 {
            return Err(resource_error("encoded schema value exceeds 16 MiB"));
        }
        Ok(())
    }
}

fn resolve_type<'a>(
    graph: &'a SchemaGraph,
    ty: &'a SchemaType,
) -> Result<&'a SchemaType, PublicSchemaValueError> {
    graph.resolve_ref(ty).map_err(|_| {
        PublicSchemaValueError::validation("schema reference cannot be resolved at this position")
    })
}

fn parse_s64(json: &Value) -> Result<i64, PublicSchemaValueError> {
    let value = json
        .as_str()
        .ok_or_else(|| PublicSchemaValueError::validation("s64 must be a decimal string"))?;
    if !canonical_signed(value) {
        return Err(PublicSchemaValueError::malformed(
            "s64 is not a canonical decimal string",
        ));
    }
    value
        .parse()
        .map_err(|_| PublicSchemaValueError::validation("s64 is out of range"))
}

fn parse_u64(json: &Value) -> Result<u64, PublicSchemaValueError> {
    let value = json
        .as_str()
        .ok_or_else(|| PublicSchemaValueError::validation("u64 must be a decimal string"))?;
    if !canonical_unsigned(value) {
        return Err(PublicSchemaValueError::malformed(
            "u64 is not a canonical decimal string",
        ));
    }
    value
        .parse()
        .map_err(|_| PublicSchemaValueError::validation("u64 is out of range"))
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

fn parse_float(json: &Value) -> Result<f64, PublicSchemaValueError> {
    let value = if let Some(value) = json.as_f64() {
        value
    } else {
        let object = exact_object(json, &["$float"])?;
        match required_string(object, "$float")? {
            "nan" => f64::NAN,
            "positive-infinity" => f64::INFINITY,
            "negative-infinity" => f64::NEG_INFINITY,
            _ => {
                return Err(PublicSchemaValueError::malformed(
                    "unknown exceptional float representation",
                ));
            }
        }
    };
    Ok(value)
}

fn encode_f32(value: f32) -> Result<Value, PublicSchemaValueError> {
    if value.is_finite() {
        Number::from_str(&value.to_string())
            .map(Value::Number)
            .map_err(|_| PublicSchemaValueError::validation("invalid finite f32"))
    } else {
        encode_float(value as f64)
    }
}

fn encode_float(value: f64) -> Result<Value, PublicSchemaValueError> {
    if value.is_nan() {
        return Ok(tagged_number_string("$float", "nan".to_string()));
    }
    if value == f64::INFINITY {
        return Ok(tagged_number_string(
            "$float",
            "positive-infinity".to_string(),
        ));
    }
    if value == f64::NEG_INFINITY {
        return Ok(tagged_number_string(
            "$float",
            "negative-infinity".to_string(),
        ));
    }
    Number::from_f64(value)
        .map(Value::Number)
        .ok_or_else(|| PublicSchemaValueError::validation("invalid finite float"))
}

fn exact_object<'a>(
    json: &'a Value,
    fields: &[&str],
) -> Result<&'a Map<String, Value>, PublicSchemaValueError> {
    let object = json
        .as_object()
        .ok_or_else(|| PublicSchemaValueError::validation("expected a JSON object"))?;
    if object.len() != fields.len()
        || object
            .keys()
            .any(|key| !fields.iter().any(|field| *field == key))
    {
        return Err(PublicSchemaValueError::validation(
            "object members do not match the expected representation",
        ));
    }
    Ok(object)
}

fn tagged_object<'a>(
    json: &'a Value,
    tag: &str,
) -> Result<&'a Map<String, Value>, PublicSchemaValueError> {
    let object = json
        .as_object()
        .ok_or_else(|| PublicSchemaValueError::validation("expected a tagged JSON object"))?;
    if !object.contains_key(tag)
        || object.len() > 2
        || object.keys().any(|key| key != tag && key != "value")
    {
        return Err(PublicSchemaValueError::validation(
            "tagged value contains invalid members",
        ));
    }
    Ok(object)
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    name: &str,
) -> Result<&'a str, PublicSchemaValueError> {
    object
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| PublicSchemaValueError::validation(format!("{name} must be a JSON string")))
}

fn array<'a>(json: &'a Value, name: &str) -> Result<&'a [Value], PublicSchemaValueError> {
    json.as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| PublicSchemaValueError::validation(format!("{name} must be an array")))
}

fn exact_array<'a>(
    json: &'a Value,
    length: usize,
    name: &str,
) -> Result<&'a [Value], PublicSchemaValueError> {
    let values = array(json, name)?;
    if values.len() != length {
        return Err(PublicSchemaValueError::validation(format!(
            "{name} has the wrong length"
        )));
    }
    Ok(values)
}

fn tagged_number_string(tag: &str, value: String) -> Value {
    let mut object = Map::new();
    object.insert(tag.to_string(), Value::String(value));
    Value::Object(object)
}

fn scalar_payload_charge(value: &SchemaValue, json: &Value) -> Result<u64, PublicSchemaValueError> {
    match value {
        SchemaValue::Bool(_) | SchemaValue::S8(_) | SchemaValue::U8(_) => Ok(1),
        SchemaValue::S16(_) | SchemaValue::U16(_) => Ok(2),
        SchemaValue::S32(_) | SchemaValue::U32(_) | SchemaValue::F32(_) => Ok(4),
        SchemaValue::S64(_) | SchemaValue::U64(_) | SchemaValue::F64(_) => Ok(8),
        SchemaValue::Char(value) => Ok(value.len_utf8() as u64),
        SchemaValue::String(value) => Ok(value.len() as u64),
        SchemaValue::Text(value) => Ok(value.text.len() as u64
            + value
                .language
                .as_ref()
                .map(|language| language.len() as u64)
                .unwrap_or_default()),
        SchemaValue::Path { path } => Ok(path.len() as u64),
        SchemaValue::Url { url } => Ok(url.len() as u64),
        SchemaValue::Datetime { .. } => json
            .as_str()
            .map(|value| value.len() as u64)
            .ok_or_else(|| PublicSchemaValueError::validation("datetime must be a string")),
        _ => Err(PublicSchemaValueError::validation(
            "value does not match the selected scalar schema",
        )),
    }
}

fn valid_mime_type(value: &str) -> bool {
    let Some((kind, subtype)) = value.split_once('/') else {
        return false;
    };
    !kind.is_empty()
        && !subtype.is_empty()
        && kind.bytes().all(valid_mime_byte)
        && subtype.bytes().all(valid_mime_byte)
}

fn valid_mime_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
        )
}

fn resource_error(message: impl Into<String>) -> PublicSchemaValueError {
    PublicSchemaValueError::new(PublicErrorCode::ResourceExhausted, message)
}

#[cfg(test)]
mod tests {
    use super::{
        PublicSchemaValueError, PublicStreamReference, PublicStreamReferencePolicy,
        decode_public_schema_value, encode_public_schema_value,
    };
    use crate::model::invocation_session_public::{
        MAX_LOGICAL_VALUE_SIZE, PublicErrorCode, encode_json_value,
    };
    use crate::schema::stream::SchemaValueStream;
    use crate::schema::{
        BinaryRestrictions, DiscriminatorRule, NamedFieldType, PathDirection, PathKind, PathSpec,
        QuantitySpec, ResultSpec, SchemaGraph, SchemaType, SchemaTypeDef, SchemaValue,
        TextRestrictions, TypeId, UnionBranch, UnionSpec, UrlRestrictions, VariantCaseType,
    };
    use serde_json::{Value, json};
    use test_r::test;

    fn decode(
        ty: &SchemaType,
        json: serde_json::Value,
        policy: PublicStreamReferencePolicy,
    ) -> Result<SchemaValue, PublicSchemaValueError> {
        decode_public_schema_value(
            &SchemaGraph::anonymous(ty.clone()),
            ty,
            &json,
            policy,
            |reference, _| Ok(SchemaValueStream::from_host_endpoint(reference)),
        )
    }

    fn fixture_schema(value: &Value) -> SchemaType {
        let object = value.as_object().expect("fixture schema must be an object");
        let kind = object["kind"]
            .as_str()
            .expect("fixture schema kind must be a string");
        match kind {
            "ref" => SchemaType::ref_to(TypeId::new(
                object["name"]
                    .as_str()
                    .expect("fixture ref must have a name"),
            )),
            "bool" => SchemaType::bool(),
            "s8" => SchemaType::s8(),
            "s16" => SchemaType::s16(),
            "s32" => SchemaType::s32(),
            "s64" => SchemaType::s64(),
            "u8" => SchemaType::u8(),
            "u16" => SchemaType::u16(),
            "u32" => SchemaType::u32(),
            "u64" => SchemaType::u64(),
            "f32" => SchemaType::f32(),
            "f64" => SchemaType::f64(),
            "char" => SchemaType::char(),
            "string" => SchemaType::string(),
            "text" => SchemaType::text(TextRestrictions::default()),
            "binary" => SchemaType::binary(BinaryRestrictions {
                mime_types: object.get("mimeTypes").map(|value| {
                    value
                        .as_array()
                        .expect("fixture MIME types must be an array")
                        .iter()
                        .map(|value| {
                            value
                                .as_str()
                                .expect("fixture MIME type must be a string")
                                .to_string()
                        })
                        .collect()
                }),
                min_bytes: object
                    .get("minBytes")
                    .and_then(Value::as_u64)
                    .map(|value| value as u32),
                max_bytes: object
                    .get("maxBytes")
                    .and_then(Value::as_u64)
                    .map(|value| value as u32),
            }),
            "path" => SchemaType::path(PathSpec {
                direction: PathDirection::Input,
                kind: PathKind::Any,
                allowed_mime_types: None,
                allowed_extensions: None,
            }),
            "url" => SchemaType::url(UrlRestrictions::default()),
            "datetime" => SchemaType::datetime(),
            "duration" => SchemaType::duration(),
            "quantity" => SchemaType::quantity(QuantitySpec {
                base_unit: "kg".to_string(),
                allowed_suffixes: Vec::new(),
                min: None,
                max: None,
            }),
            "record" => SchemaType::record(
                object["fields"]
                    .as_array()
                    .expect("fixture record fields must be an array")
                    .iter()
                    .map(|field| NamedFieldType {
                        name: field["name"]
                            .as_str()
                            .expect("fixture field must have a name")
                            .to_string(),
                        body: fixture_schema(&field["type"]),
                        metadata: Default::default(),
                    })
                    .collect(),
            ),
            "tuple" => SchemaType::tuple(
                object["elements"]
                    .as_array()
                    .expect("fixture tuple elements must be an array")
                    .iter()
                    .map(fixture_schema)
                    .collect(),
            ),
            "list" => SchemaType::list(fixture_schema(&object["element"])),
            "fixed-list" => SchemaType::fixed_list(
                fixture_schema(&object["element"]),
                object["length"]
                    .as_u64()
                    .expect("fixture fixed-list length must be an integer") as u32,
            ),
            "map" => SchemaType::map(
                fixture_schema(&object["key"]),
                fixture_schema(&object["value"]),
            ),
            "enum" => SchemaType::r#enum(fixture_strings(&object["cases"])),
            "flags" => SchemaType::flags(fixture_strings(&object["flags"])),
            "variant" => SchemaType::variant(
                object["cases"]
                    .as_array()
                    .expect("fixture variant cases must be an array")
                    .iter()
                    .map(|case| VariantCaseType {
                        name: case["name"]
                            .as_str()
                            .expect("fixture variant case must have a name")
                            .to_string(),
                        payload: case.get("type").map(fixture_schema),
                        metadata: Default::default(),
                    })
                    .collect(),
            ),
            "option" => SchemaType::option(fixture_schema(&object["inner"])),
            "result" => SchemaType::result(ResultSpec {
                ok: object
                    .get("ok")
                    .filter(|value| !value.is_null())
                    .map(|value| Box::new(fixture_schema(value))),
                err: object
                    .get("err")
                    .filter(|value| !value.is_null())
                    .map(|value| Box::new(fixture_schema(value))),
            }),
            "union" => SchemaType::union(UnionSpec {
                branches: object["branches"]
                    .as_array()
                    .expect("fixture union branches must be an array")
                    .iter()
                    .map(|branch| {
                        let discriminator = &branch["discriminator"];
                        assert_eq!(discriminator["rule"], "prefix");
                        UnionBranch {
                            tag: branch["name"]
                                .as_str()
                                .expect("fixture union branch must have a name")
                                .to_string(),
                            body: fixture_schema(&branch["type"]),
                            discriminator: DiscriminatorRule::Prefix {
                                prefix: discriminator["prefix"]
                                    .as_str()
                                    .expect("fixture prefix must be a string")
                                    .to_string(),
                            },
                            metadata: Default::default(),
                        }
                    })
                    .collect(),
            }),
            "stream" => SchemaType::stream(
                object
                    .get("inner")
                    .filter(|value| !value.is_null())
                    .map(fixture_schema),
            ),
            other => panic!("unknown fixture schema kind {other}"),
        }
    }

    fn fixture_strings(value: &Value) -> Vec<String> {
        value
            .as_array()
            .expect("fixture strings must be an array")
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .expect("fixture value must be a string")
                    .to_string()
            })
            .collect()
    }

    fn fixture_graph(vector: &Value) -> SchemaGraph {
        let root = fixture_schema(&vector["schema"]);
        let defs = vector
            .get("definitions")
            .and_then(Value::as_object)
            .map(|definitions| {
                definitions
                    .iter()
                    .map(|(name, body)| SchemaTypeDef {
                        id: TypeId::new(name),
                        name: Some(name.clone()),
                        body: fixture_schema(body),
                    })
                    .collect()
            })
            .unwrap_or_default();
        SchemaGraph { defs, root }
    }

    fn expected_error_code(value: &str) -> PublicErrorCode {
        match value {
            "malformed-message" => PublicErrorCode::MalformedMessage,
            "validation-error" => PublicErrorCode::ValidationError,
            "stream-already-consumed" => PublicErrorCode::StreamAlreadyConsumed,
            other => panic!("unknown fixture error code {other}"),
        }
    }

    #[test]
    fn every_schema_value_fixture_round_trips_canonically() {
        let fixture: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../golem-client/tests/fixtures/stream-session-v1/schema-values.json"
        )))
        .unwrap();
        assert_eq!(fixture["version"], 1);
        let vectors = fixture["vectors"].as_array().unwrap();
        assert!(!vectors.is_empty());

        for vector in vectors {
            let name = vector["name"].as_str().unwrap();
            let canonical = vector["canonical"].as_str().unwrap();
            let json: Value = serde_json::from_str(canonical).unwrap();
            let graph = fixture_graph(vector);
            let policy = if canonical.contains("provisionalRef") {
                PublicStreamReferencePolicy::Provisional
            } else if canonical.contains("streamToken") {
                PublicStreamReferencePolicy::Stable
            } else {
                PublicStreamReferencePolicy::None
            };
            let value =
                decode_public_schema_value(&graph, &graph.root, &json, policy, |reference, _| {
                    Ok(SchemaValueStream::from_host_endpoint(reference))
                })
                .unwrap_or_else(|error| panic!("fixture {name} did not decode: {error}"));
            let encoded = encode_public_schema_value(&graph, &graph.root, &value, |stream, _| {
                stream
                    .take_host_endpoint::<PublicStreamReference>()
                    .map_err(|_| {
                        PublicSchemaValueError::validation(
                            "fixture stream reference was already consumed",
                        )
                    })
            })
            .unwrap_or_else(|error| panic!("fixture {name} did not encode: {error}"));
            assert_eq!(
                encode_json_value(&encoded).unwrap(),
                canonical,
                "fixture {name} changed canonical representation"
            );
        }
    }

    #[test]
    fn every_malformed_schema_value_fixture_has_the_frozen_error_code() {
        let fixture: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../golem-client/tests/fixtures/stream-session-v1/malformed.json"
        )))
        .unwrap();
        let vectors = fixture["vectors"].as_array().unwrap();
        let schema_vectors = vectors
            .iter()
            .filter(|vector| vector["lane"] == "schema-value")
            .collect::<Vec<_>>();
        assert!(!schema_vectors.is_empty());

        for vector in schema_vectors {
            let name = vector["name"].as_str().unwrap();
            let graph = fixture_graph(vector);
            let input: Value = serde_json::from_str(vector["input"].as_str().unwrap()).unwrap();
            let policy = if vector["input"].as_str().unwrap().contains("provisionalRef") {
                PublicStreamReferencePolicy::Provisional
            } else {
                PublicStreamReferencePolicy::None
            };
            let error =
                decode_public_schema_value(&graph, &graph.root, &input, policy, |reference, _| {
                    Ok(SchemaValueStream::from_host_endpoint(reference))
                })
                .unwrap_err();
            assert_eq!(
                error.code,
                expected_error_code(vector["expectedCode"].as_str().unwrap()),
                "fixture {name} returned the wrong public error code"
            );
        }
    }

    #[test]
    fn safe_integers_and_standard_base64_round_trip() {
        assert_eq!(
            decode(
                &SchemaType::u64(),
                json!("18446744073709551615"),
                PublicStreamReferencePolicy::None,
            )
            .unwrap(),
            SchemaValue::U64(u64::MAX)
        );
        assert!(
            decode(
                &SchemaType::u64(),
                json!(18446744073709551615u64),
                PublicStreamReferencePolicy::None,
            )
            .is_err()
        );
        assert_eq!(
            decode(
                &SchemaType::binary(BinaryRestrictions::default()),
                json!({"bytes":"+/8=","mimeType":"application/octet-stream"}),
                PublicStreamReferencePolicy::None,
            )
            .unwrap(),
            SchemaValue::Binary(crate::schema::BinaryValuePayload {
                bytes: vec![251, 255],
                mime_type: Some("application/octet-stream".to_string()),
            })
        );
    }

    #[test]
    fn tagged_structures_round_trip() {
        let ty = SchemaType::record(vec![NamedFieldType {
            name: "value".to_string(),
            body: SchemaType::option(SchemaType::result(ResultSpec {
                ok: Some(Box::new(SchemaType::u64())),
                err: None,
            })),
            metadata: Default::default(),
        }]);
        let graph = SchemaGraph::anonymous(ty.clone());
        let json = json!({
            "value":{"$option":"some","value":{"$result":"ok","value":"42"}}
        });
        let value = decode_public_schema_value(
            &graph,
            &ty,
            &json,
            PublicStreamReferencePolicy::None,
            |_, _| unreachable!(),
        )
        .unwrap();
        let encoded =
            encode_public_schema_value(&graph, &ty, &value, |_, _| unreachable!()).unwrap();
        assert_eq!(encoded, json);
    }

    #[test]
    fn floating_point_canonical_forms_are_preserved() {
        let graph = SchemaGraph::anonymous(SchemaType::f64());
        let encoded = encode_public_schema_value(
            &graph,
            &graph.root,
            &SchemaValue::F64(-0.0),
            |_, _| unreachable!(),
        )
        .unwrap();
        assert_eq!(encode_json_value(&encoded).unwrap(), "-0");

        let graph = SchemaGraph::anonymous(SchemaType::f32());
        let encoded = encode_public_schema_value(
            &graph,
            &graph.root,
            &SchemaValue::F32(0.1),
            |_, _| unreachable!(),
        )
        .unwrap();
        assert_eq!(encode_json_value(&encoded).unwrap(), "0.1");
    }

    #[test]
    fn duplicate_provisional_stream_is_an_affine_violation() {
        let ty = SchemaType::list(SchemaType::stream(Some(SchemaType::u8())));
        let reference = "0dff1c71-f12f-4bb1-996c-23d693bdc825";
        let error = decode(
            &ty,
            json!([
                {"$stream":{"provisionalRef":reference}},
                {"$stream":{"provisionalRef":reference}}
            ]),
            PublicStreamReferencePolicy::Provisional,
        )
        .unwrap_err();
        assert_eq!(error.code, PublicErrorCode::StreamAlreadyConsumed);
    }

    #[test]
    fn provisional_stream_references_require_canonical_rfc4122_uuid_v4() {
        let ty = SchemaType::stream(Some(SchemaType::u8()));
        for reference in [
            "00000000-0000-1000-8000-000000000000",
            "00000000-0000-4000-0000-000000000000",
            "0DFF1C71-F12F-4BB1-996C-23D693BDC825",
        ] {
            let error = decode(
                &ty,
                json!({"$stream":{"provisionalRef":reference}}),
                PublicStreamReferencePolicy::Provisional,
            )
            .unwrap_err();
            assert_eq!(error.code, PublicErrorCode::ValidationError);
        }
    }

    #[test]
    fn stream_reference_policy_is_directional() {
        let ty = SchemaType::stream(Some(SchemaType::u8()));
        let graph = SchemaGraph::anonymous(ty.clone());
        let value = decode_public_schema_value(
            &graph,
            &ty,
            &json!({"$stream":{"streamToken":"opaque-stream-token"}}),
            PublicStreamReferencePolicy::Stable,
            |reference, _| {
                assert_eq!(
                    reference,
                    PublicStreamReference::Stable("opaque-stream-token".to_string())
                );
                Ok(SchemaValueStream::from_host_endpoint(()))
            },
        )
        .unwrap();
        assert!(matches!(value, SchemaValue::Stream(_)));
    }

    #[test]
    fn logical_value_byte_limit_is_enforced_for_decode_and_encode() {
        let ty = SchemaType::string();
        let graph = SchemaGraph::anonymous(ty.clone());
        let boundary = "x".repeat(MAX_LOGICAL_VALUE_SIZE - 1);
        let oversized = "x".repeat(MAX_LOGICAL_VALUE_SIZE);

        assert!(
            decode_public_schema_value(
                &graph,
                &ty,
                &json!(boundary),
                PublicStreamReferencePolicy::None,
                |_, _| unreachable!(),
            )
            .is_ok()
        );
        let decode_error = decode_public_schema_value(
            &graph,
            &ty,
            &json!(oversized.clone()),
            PublicStreamReferencePolicy::None,
            |_, _| unreachable!(),
        )
        .unwrap_err();
        assert_eq!(decode_error.code, PublicErrorCode::ResourceExhausted);

        let encode_error = encode_public_schema_value(
            &graph,
            &ty,
            &SchemaValue::String(oversized),
            |_, _| unreachable!(),
        )
        .unwrap_err();
        assert_eq!(encode_error.code, PublicErrorCode::ResourceExhausted);
    }

    #[test]
    fn malformed_schema_value_vectors_have_frozen_error_codes() {
        let vectors = [
            (
                SchemaType::u64(),
                json!(9007199254740993u64),
                PublicErrorCode::ValidationError,
            ),
            (
                SchemaType::binary(BinaryRestrictions::default()),
                json!({"bytes":"-_8=","mimeType":"application/octet-stream"}),
                PublicErrorCode::MalformedMessage,
            ),
            (
                SchemaType::binary(BinaryRestrictions::default()),
                json!({"bytes":"+/8","mimeType":"application/octet-stream"}),
                PublicErrorCode::MalformedMessage,
            ),
            (
                SchemaType::f64(),
                json!({"$float":"infinity"}),
                PublicErrorCode::MalformedMessage,
            ),
        ];
        for (ty, value, expected) in vectors {
            let error = decode(&ty, value, PublicStreamReferencePolicy::None).unwrap_err();
            assert_eq!(error.code, expected);
        }
    }
}
