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

impl<T: AllowedLanguages> Schema for UnstructuredText<T> {
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

        StructuredSchema::Default(crate::agentic::unstructured_text_schema_graph(restrictions))
    }

    fn to_schema_value(self) -> Result<SchemaValue, String> {
        match self {
            UnstructuredText::Text {
                text,
                language_code,
            } => Ok(crate::agentic::unstructured_text_inline_value(
                text,
                language_code.map(|code| code.to_language_code()),
            )),

            UnstructuredText::Url(url) => Ok(crate::agentic::unstructured_text_url_value(url)),
        }
    }

    fn from_schema_value(value: SchemaValue, _schema: StructuredSchema) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            SchemaValue::Variant(VariantValuePayload { case: 0, payload }) => {
                match payload.map(|payload| *payload) {
                    Some(SchemaValue::Text(payload)) => Self::from_text_payload(payload),
                    Some(other) => Err(format!(
                        "Expected inline UnstructuredText payload, got: {:?}",
                        other
                    )),
                    None => Err("Missing inline UnstructuredText payload".to_string()),
                }
            }
            SchemaValue::Variant(VariantValuePayload { case: 1, payload }) => {
                match payload.map(|payload| *payload) {
                    Some(SchemaValue::Url { url }) => Ok(UnstructuredText::Url(url)),
                    Some(other) => Err(format!(
                        "Expected UnstructuredText URL payload, got: {:?}",
                        other
                    )),
                    None => Err("Missing UnstructuredText URL payload".to_string()),
                }
            }
            other => Err(format!(
                "Expected UnstructuredText schema value, got: {:?}",
                other
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
