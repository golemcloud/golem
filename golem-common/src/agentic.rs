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

//! Host-side, schema-native ergonomic wrappers for agent unstructured
//! text/binary values, used by the generated Rust agent bridge.
//!
//! These mirror the guest SDK's `golem_rust::agentic::{UnstructuredText,
//! UnstructuredBinary}`, but are defined here so the host-side generated bridge
//! crate — which depends on `golem-common`, not the guest SDK — can use the same
//! ergonomic API. They encode/decode directly to/from the schema-native
//! [`SchemaValue`] wire form: a two-case variant (`0 = inline`, `1 = url`).

use crate::schema::{BinaryValuePayload, SchemaValue, TextValuePayload, VariantValuePayload};
use std::fmt::Debug;

/// Represents a text value that can either be inline or a URL reference.
///
/// `LC` specifies the allowed language codes for inline text. Defaults to
/// [`AnyLanguage`], which allows all languages.
#[derive(Debug, Clone)]
pub enum UnstructuredText<LC: AllowedLanguages = AnyLanguage> {
    Url(String),
    Text {
        text: String,
        language_code: Option<LC>,
    },
}

impl<T: AllowedLanguages> UnstructuredText<T> {
    pub fn from_inline(text: impl Into<String>, language_code: T) -> UnstructuredText<T> {
        UnstructuredText::Text {
            text: text.into(),
            language_code: Some(language_code),
        }
    }
}

impl UnstructuredText<AnyLanguage> {
    pub fn from_url_any(url: impl Into<String>) -> UnstructuredText<AnyLanguage> {
        UnstructuredText::Url(url.into())
    }

    pub fn from_inline_any(text: impl Into<String>) -> UnstructuredText<AnyLanguage> {
        UnstructuredText::Text {
            text: text.into(),
            language_code: None,
        }
    }
}

impl<T: AllowedLanguages> UnstructuredText<T> {
    /// Encodes this value into its schema-native [`SchemaValue`] wire form.
    pub fn to_schema_value(&self) -> Result<SchemaValue, String> {
        match self {
            UnstructuredText::Text {
                text,
                language_code,
            } => Ok(SchemaValue::Variant(VariantValuePayload {
                case: 0,
                payload: Some(Box::new(SchemaValue::Text(TextValuePayload {
                    text: text.clone(),
                    language: language_code.as_ref().map(|code| code.to_language_code()),
                }))),
            })),
            UnstructuredText::Url(url) => Ok(SchemaValue::Variant(VariantValuePayload {
                case: 1,
                payload: Some(Box::new(SchemaValue::Url { url: url.clone() })),
            })),
        }
    }

    /// Decodes a schema-native [`SchemaValue`] wire value back into the wrapper.
    pub fn from_schema_value(value: SchemaValue) -> Result<Self, String> {
        match value {
            SchemaValue::Variant(VariantValuePayload { case: 0, payload }) => {
                match payload.map(|payload| *payload) {
                    Some(SchemaValue::Text(payload)) => Self::from_text_payload(payload),
                    Some(other) => Err(format!(
                        "Expected inline UnstructuredText payload, got: {other:?}"
                    )),
                    None => Err("Missing inline UnstructuredText payload".to_string()),
                }
            }
            SchemaValue::Variant(VariantValuePayload { case: 1, payload }) => {
                match payload.map(|payload| *payload) {
                    Some(SchemaValue::Url { url }) => Ok(UnstructuredText::Url(url)),
                    Some(other) => {
                        Err(format!("Expected UnstructuredText URL payload, got: {other:?}"))
                    }
                    None => Err("Missing UnstructuredText URL payload".to_string()),
                }
            }
            other => Err(format!("Expected UnstructuredText schema value, got: {other:?}")),
        }
    }

    fn from_text_payload(payload: TextValuePayload) -> Result<Self, String> {
        let language_code = match payload.language {
            Some(code) => Some(T::from_language_code(&code).ok_or_else(|| {
                format!("Language code '{code}' is not allowed for UnstructuredText")
            })?),
            None => None,
        };

        Ok(UnstructuredText::Text {
            text: payload.text,
            language_code,
        })
    }
}

/// Trait implemented by the set of language codes allowed for an
/// [`UnstructuredText`] value. The generated bridge emits one implementor enum
/// per distinct language restriction set.
pub trait AllowedLanguages: Debug + Clone {
    fn all() -> &'static [&'static str];

    fn from_language_code(code: &str) -> Option<Self>
    where
        Self: Sized;

    fn to_language_code(&self) -> String;
}

/// Language placeholder that accepts any BCP-47 language tag.
#[derive(Debug, Clone)]
pub struct AnyLanguage(String);

impl AnyLanguage {
    pub fn new(code: impl Into<String>) -> Self {
        Self(code.into())
    }
}

impl AllowedLanguages for AnyLanguage {
    fn all() -> &'static [&'static str] {
        &[]
    }

    fn from_language_code(code: &str) -> Option<Self> {
        Some(Self::new(code))
    }

    fn to_language_code(&self) -> String {
        self.0.clone()
    }
}

/// Represents a binary value that can either be inline or a URL reference.
///
/// `MT` specifies the allowed MIME types for inline binary data. Defaults to
/// [`String`], which allows any MIME type.
#[derive(Debug, Clone)]
pub enum UnstructuredBinary<MT: AllowedMimeTypes = String> {
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

    /// Encodes this value into its schema-native [`SchemaValue`] wire form.
    pub fn to_schema_value(&self) -> Result<SchemaValue, String> {
        match self {
            UnstructuredBinary::Inline { data, mime_type } => {
                Ok(SchemaValue::Variant(VariantValuePayload {
                    case: 0,
                    payload: Some(Box::new(SchemaValue::Binary(BinaryValuePayload {
                        bytes: data.clone(),
                        mime_type: Some(mime_type.to_mime_type()),
                    }))),
                }))
            }
            UnstructuredBinary::Url(url) => Ok(SchemaValue::Variant(VariantValuePayload {
                case: 1,
                payload: Some(Box::new(SchemaValue::Url { url: url.clone() })),
            })),
        }
    }

    /// Decodes a schema-native [`SchemaValue`] wire value back into the wrapper.
    pub fn from_schema_value(value: SchemaValue) -> Result<Self, String> {
        match value {
            SchemaValue::Variant(VariantValuePayload { case: 0, payload }) => {
                match payload.map(|payload| *payload) {
                    Some(SchemaValue::Binary(payload)) => {
                        Self::from_binary_payload(payload.bytes, payload.mime_type)
                    }
                    Some(other) => Err(format!(
                        "Expected inline UnstructuredBinary payload, got: {other:?}"
                    )),
                    None => Err("Missing inline UnstructuredBinary payload".to_string()),
                }
            }
            SchemaValue::Variant(VariantValuePayload { case: 1, payload }) => {
                match payload.map(|payload| *payload) {
                    Some(SchemaValue::Url { url }) => Ok(UnstructuredBinary::Url(url)),
                    Some(other) => Err(format!(
                        "Expected UnstructuredBinary URL payload, got: {other:?}"
                    )),
                    None => Err("Missing UnstructuredBinary URL payload".to_string()),
                }
            }
            other => Err(format!(
                "Expected UnstructuredBinary schema value, got: {other:?}"
            )),
        }
    }

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

        let mime_type = T::from_mime_type(&mime_type).ok_or_else(|| {
            format!("Failed to convert mime type '{mime_type}' to AllowedMimeType")
        })?;

        Ok(UnstructuredBinary::Inline { data, mime_type })
    }
}

/// Trait implemented by the set of MIME types allowed for an
/// [`UnstructuredBinary`] value. The generated bridge emits one implementor
/// enum per distinct MIME-type restriction set.
pub trait AllowedMimeTypes: Debug + Clone {
    fn all() -> &'static [&'static str];

    fn from_mime_type(mime_type: &str) -> Option<Self>
    where
        Self: Sized;

    fn to_mime_type(&self) -> String;
}

impl AllowedMimeTypes for String {
    fn all() -> &'static [&'static str] {
        &[]
    }

    fn from_mime_type(mime_type: &str) -> Option<Self> {
        Some(mime_type.to_string())
    }

    fn to_mime_type(&self) -> String {
        self.clone()
    }
}
