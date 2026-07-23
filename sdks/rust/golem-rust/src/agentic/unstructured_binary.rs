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

use crate::SchemaValue;
use crate::agentic::{Schema, StructuredSchema};
use crate::schema::VariantValuePayload;
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

impl<T: AllowedMimeTypes> Schema for UnstructuredBinary<T> {
    fn get_type() -> StructuredSchema {
        let restrictions = if T::all().is_empty() {
            None
        } else {
            let restrictions = T::all()
                .iter()
                .map(|code| code.to_string())
                .collect::<Vec<_>>();

            Some(restrictions)
        };

        StructuredSchema::Default(crate::agentic::unstructured_binary_schema_graph(
            restrictions,
        ))
    }

    fn to_schema_value(self) -> Result<SchemaValue, String> {
        match self {
            UnstructuredBinary::Inline { data, mime_type } => Ok(
                crate::agentic::unstructured_binary_inline_value(data, Some(mime_type.to_string())),
            ),

            UnstructuredBinary::Url(url) => Ok(crate::agentic::unstructured_binary_url_value(url)),
        }
    }

    fn from_schema_value(value: SchemaValue, _schema: StructuredSchema) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            SchemaValue::Variant(VariantValuePayload { case: 0, payload }) => {
                match payload.map(|payload| *payload) {
                    Some(SchemaValue::Binary(payload)) => {
                        Self::from_binary_payload(payload.bytes, payload.mime_type)
                    }
                    Some(other) => Err(format!(
                        "Expected inline UnstructuredBinary payload, got: {:?}",
                        other
                    )),
                    None => Err("Missing inline UnstructuredBinary payload".to_string()),
                }
            }
            SchemaValue::Variant(VariantValuePayload { case: 1, payload }) => {
                match payload.map(|payload| *payload) {
                    Some(SchemaValue::Url { url }) => Ok(UnstructuredBinary::Url(url)),
                    Some(other) => Err(format!(
                        "Expected UnstructuredBinary URL payload, got: {:?}",
                        other
                    )),
                    None => Err("Missing UnstructuredBinary URL payload".to_string()),
                }
            }
            other => Err(format!(
                "Expected UnstructuredBinary schema value, got: {:?}",
                other
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
