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

pub trait AllowedLanguages: Debug + Clone {
    fn all() -> &'static [&'static str];

    fn from_language_code(code: &str) -> Option<Self>
    where
        Self: Sized;

    fn to_language_code(&self) -> String;
}

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

impl<T: AllowedLanguages> IntoSchema for UnstructuredText<T> {
    fn type_id() -> TypeId {
        default_type_id_from(std::any::type_name::<Self>())
    }

    fn register_in(_builder: &mut SchemaBuilder) -> SchemaType {
        crate::agentic::unstructured_text_schema_type(
            (!T::all().is_empty()).then(|| T::all().iter().map(|s| (*s).to_string()).collect()),
        )
    }

    fn to_value(&self) -> SchemaValue {
        match self {
            UnstructuredText::Text {
                text,
                language_code,
            } => crate::agentic::unstructured_text_inline_value(
                text.clone(),
                language_code.as_ref().map(T::to_language_code),
            ),
            UnstructuredText::Url(url) => crate::agentic::unstructured_text_url_value(url.clone()),
        }
    }
}

impl<T: AllowedLanguages> FromSchema for UnstructuredText<T> {
    fn from_value(value: &SchemaValue) -> Result<Self, FromSchemaError> {
        match value {
            SchemaValue::Variant(VariantValuePayload { case: 0, payload }) => {
                match payload.as_deref() {
                    Some(SchemaValue::Text(payload)) => {
                        Self::from_text_payload(payload.clone()).map_err(FromSchemaError::custom)
                    }
                    Some(other) => Err(FromSchemaError::shape_mismatch(
                        "text",
                        value_kind(other),
                        "UnstructuredText inline payload",
                    )),
                    None => Err(FromSchemaError::custom(
                        "Missing inline UnstructuredText payload",
                    )),
                }
            }
            SchemaValue::Variant(VariantValuePayload { case: 1, payload }) => {
                match payload.as_deref() {
                    Some(SchemaValue::Url { url }) => Ok(UnstructuredText::Url(url.clone())),
                    Some(other) => Err(FromSchemaError::shape_mismatch(
                        "url",
                        value_kind(other),
                        "UnstructuredText URL payload",
                    )),
                    None => Err(FromSchemaError::custom(
                        "Missing UnstructuredText URL payload",
                    )),
                }
            }
            SchemaValue::Variant(VariantValuePayload { case, .. }) => {
                Err(FromSchemaError::out_of_range(*case, 2, "UnstructuredText"))
            }
            other => Err(FromSchemaError::shape_mismatch(
                "variant { inline, url }",
                value_kind(other),
                "UnstructuredText",
            )),
        }
    }
}

impl<T: AllowedLanguages> UnstructuredText<T> {
    fn from_text_payload(payload: crate::schema::TextValuePayload) -> Result<Self, String> {
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

impl<T: AllowedLanguages> direct::WireSchema for UnstructuredText<T> {
    fn append_schema(builder: &mut direct::WireSchemaBuilder) -> i32 {
        let inline = builder.push(wire::SchemaTypeBody::TextType(wire::TextRestrictions {
            languages: (!T::all().is_empty())
                .then(|| T::all().iter().map(|s| (*s).to_string()).collect()),
            min_length: None,
            max_length: None,
            regex: None,
        }));
        let url = builder.push(wire::SchemaTypeBody::UrlType(wire::UrlRestrictions {
            allowed_schemes: None,
            allowed_hosts: None,
        }));
        let mut metadata = direct::empty_metadata();
        metadata.role = Some(wire::Role::UnstructuredText);
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

impl<T: AllowedLanguages> direct::IntoWire for UnstructuredText<T> {
    fn write_wire(&self, writer: &mut direct::WireWriter) -> Result<i32, direct::WireError> {
        let (case, payload) = match self {
            Self::Url(url) => (1, wire::SchemaValueNode::UrlValue(url.clone())),
            Self::Text {
                text,
                language_code,
            } => (
                0,
                wire::SchemaValueNode::TextValue(wire::TextValuePayload {
                    text: text.clone(),
                    language: language_code.as_ref().map(T::to_language_code),
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

impl<T: AllowedLanguages> direct::FromWire for UnstructuredText<T> {
    fn read_wire(reader: &mut direct::WireReader, index: i32) -> Result<Self, direct::WireError> {
        let wire::SchemaValueNode::VariantValue(wire::VariantValuePayload {
            case,
            payload: Some(payload),
        }) = reader.take(index)?
        else {
            return Err(direct::WireError::Shape(
                "unstructured text variant with payload",
            ));
        };
        match (case, reader.take(payload)?) {
            (0, wire::SchemaValueNode::TextValue(payload)) => Ok(Self::Text {
                text: payload.text,
                language_code: payload
                    .language
                    .map(|code| {
                        T::from_language_code(&code)
                            .ok_or(direct::WireError::Shape("allowed text language"))
                    })
                    .transpose()?,
            }),
            (1, wire::SchemaValueNode::UrlValue(url)) => Ok(Self::Url(url)),
            _ => Err(direct::WireError::Shape("unstructured text payload")),
        }
    }
}
