// Copyright 2024-2026 Golem Cloud
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

use crate::schema::conversion::{default_type_id_from, value_kind};
use crate::schema::wit::{direct, wire};
use crate::schema::{
    FromSchema, FromSchemaError, IntoSchema, SchemaBuilder, SchemaType, SchemaValue, TypeId,
    VariantValuePayload,
};
use std::fmt::Debug;

/// Represents a binary value that can either be inline or a URL reference.
///
/// `MT` specifies the allowed MIME types for inline binary data.
#[derive(Debug, Clone)]
pub enum UnstructuredBinary<MT: AllowedMimeTypes> {
    Url(String),
    Inline { data: Vec<u8>, mime_type: MT },
}

impl<T: AllowedMimeTypes> UnstructuredBinary<T> {
    pub fn from_inline(data: Vec<u8>, mime_type: T) -> UnstructuredBinary<T> {
        UnstructuredBinary::Inline { data, mime_type }
    }

    pub fn from_url(url: impl Into<String>) -> UnstructuredBinary<T> {
        UnstructuredBinary::Url(url.into())
    }
}

pub trait AllowedMimeTypes: Debug + Clone {
    fn all() -> &'static [&'static str];

    fn from_string(mime_type: &str) -> Option<Self>
    where
        Self: Sized;

    fn to_string(&self) -> String;
}

impl AllowedMimeTypes for String {
    fn all() -> &'static [&'static str] {
        &[]
    }

    fn from_string(mime_type: &str) -> Option<Self> {
        Some(mime_type.to_string())
    }

    fn to_string(&self) -> String {
        self.clone()
    }
}

impl<T: AllowedMimeTypes> IntoSchema for UnstructuredBinary<T> {
    fn type_id() -> TypeId {
        default_type_id_from(std::any::type_name::<Self>())
    }

    fn register_in(_builder: &mut SchemaBuilder) -> SchemaType {
        crate::agentic::unstructured_binary_schema_type(
            (!T::all().is_empty()).then(|| T::all().iter().map(|s| (*s).to_string()).collect()),
        )
    }

    fn to_value(&self) -> SchemaValue {
        match self {
            UnstructuredBinary::Inline { data, mime_type } => {
                crate::agentic::unstructured_binary_inline_value(
                    data.clone(),
                    Some(mime_type.to_string()),
                )
            }
            UnstructuredBinary::Url(url) => {
                crate::agentic::unstructured_binary_url_value(url.clone())
            }
        }
    }
}

impl<T: AllowedMimeTypes> FromSchema for UnstructuredBinary<T> {
    fn from_value(value: &SchemaValue) -> Result<Self, FromSchemaError> {
        match value {
            SchemaValue::Variant(VariantValuePayload { case: 0, payload }) => {
                match payload.as_deref() {
                    Some(SchemaValue::Binary(payload)) => {
                        Self::from_binary_payload(payload.bytes.clone(), payload.mime_type.clone())
                            .map_err(FromSchemaError::custom)
                    }
                    Some(other) => Err(FromSchemaError::shape_mismatch(
                        "binary",
                        value_kind(other),
                        "UnstructuredBinary inline payload",
                    )),
                    None => Err(FromSchemaError::custom(
                        "Missing inline UnstructuredBinary payload",
                    )),
                }
            }
            SchemaValue::Variant(VariantValuePayload { case: 1, payload }) => {
                match payload.as_deref() {
                    Some(SchemaValue::Url { url }) => Ok(UnstructuredBinary::Url(url.clone())),
                    Some(other) => Err(FromSchemaError::shape_mismatch(
                        "url",
                        value_kind(other),
                        "UnstructuredBinary URL payload",
                    )),
                    None => Err(FromSchemaError::custom(
                        "Missing UnstructuredBinary URL payload",
                    )),
                }
            }
            SchemaValue::Variant(VariantValuePayload { case, .. }) => Err(
                FromSchemaError::out_of_range(*case, 2, "UnstructuredBinary"),
            ),
            other => Err(FromSchemaError::shape_mismatch(
                "variant { inline, url }",
                value_kind(other),
                "UnstructuredBinary",
            )),
        }
    }
}

impl<T: AllowedMimeTypes> UnstructuredBinary<T> {
    fn from_binary_payload(data: Vec<u8>, mime_type: Option<String>) -> Result<Self, String> {
        let allowed = T::all().iter().map(|s| s.to_string()).collect::<Vec<_>>();

        let mime_type = mime_type
            .or_else(|| allowed.first().cloned())
            .unwrap_or_default();

        if !allowed.is_empty() && !allowed.contains(&mime_type) {
            return Err(format!(
                "Mime type '{}' is not allowed. Allowed mime types: {:?}",
                mime_type,
                allowed.join(",")
            ));
        }

        let mime_type = T::from_string(&mime_type).ok_or_else(|| {
            format!(
                "Failed to convert mime type '{}' to AllowedMimeType",
                mime_type
            )
        })?;

        Ok(UnstructuredBinary::Inline { data, mime_type })
    }
}

impl<T: AllowedMimeTypes> direct::WireSchema for UnstructuredBinary<T> {
    fn append_schema(builder: &mut direct::WireSchemaBuilder) -> i32 {
        let inline = builder.push(wire::SchemaTypeBody::BinaryType(wire::BinaryRestrictions {
            mime_types: (!T::all().is_empty())
                .then(|| T::all().iter().map(|s| (*s).to_string()).collect()),
            min_bytes: None,
            max_bytes: None,
        }));
        let url = builder.push(wire::SchemaTypeBody::UrlType(wire::UrlRestrictions {
            allowed_schemes: None,
            allowed_hosts: None,
        }));
        let mut metadata = direct::empty_metadata();
        metadata.role = Some(wire::Role::UnstructuredBinary);
        builder.push_with_metadata(
            wire::SchemaTypeBody::VariantType(vec![
                wire::VariantCaseType {
                    name: "inline".to_string(),
                    payload: Some(inline),
                    metadata: direct::empty_metadata(),
                },
                wire::VariantCaseType {
                    name: "url".to_string(),
                    payload: Some(url),
                    metadata: direct::empty_metadata(),
                },
            ]),
            metadata,
        )
    }
}

impl<T: AllowedMimeTypes> direct::IntoWire for UnstructuredBinary<T> {
    fn write_wire(&self, writer: &mut direct::WireWriter) -> Result<i32, direct::WireError> {
        let (case, payload) = match self {
            Self::Url(url) => (1, wire::SchemaValueNode::UrlValue(url.clone())),
            Self::Inline { data, mime_type } => (
                0,
                wire::SchemaValueNode::BinaryValue(wire::BinaryValuePayload {
                    bytes: data.clone(),
                    mime_type: Some(mime_type.to_string()),
                }),
            ),
        };
        let payload = writer.push(payload);
        Ok(writer.push(wire::SchemaValueNode::VariantValue(
            wire::VariantValuePayload {
                case,
                payload: Some(payload),
            },
        )))
    }
}

impl<T: AllowedMimeTypes> direct::FromWire for UnstructuredBinary<T> {
    fn read_wire(reader: &mut direct::WireReader, index: i32) -> Result<Self, direct::WireError> {
        let wire::SchemaValueNode::VariantValue(wire::VariantValuePayload {
            case,
            payload: Some(payload),
        }) = reader.take(index)?
        else {
            return Err(direct::WireError::Shape(
                "unstructured binary variant with payload",
            ));
        };
        match (case, reader.take(payload)?) {
            (0, wire::SchemaValueNode::BinaryValue(payload)) => {
                Self::from_binary_payload(payload.bytes, payload.mime_type)
                    .map_err(|_| direct::WireError::Shape("allowed binary MIME type"))
            }
            (1, wire::SchemaValueNode::UrlValue(url)) => Ok(Self::Url(url)),
            _ => Err(direct::WireError::Shape("unstructured binary payload")),
        }
    }
}
