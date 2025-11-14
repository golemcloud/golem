// Copyright 2024-2025 Golem Cloud
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

use crate::agentic::Schema;
use crate::golem_agentic::golem::agent::common::{BinaryDescriptor, BinaryReference, BinarySource, BinaryType, DataSchema, ElementSchema, ElementValue, TextDescriptor, TextReference, TextSource, TextType, Url};

/// Represents a binary value that can either be inline or a URL reference.
///
/// `LC` specifies the allowed language codes for inline text. Defaults to `AnyLanguage`,
/// which allows all languages.
pub enum UnstructuredBinary<MT: AllowedMimeType> {
    Url(String),
    Inline {
        data: Vec<u8>,
        mime_type: MT,
    },
}

impl<T: AllowedMimeType> UnstructuredBinary<T> {
    /// Create an `UnstructuredBinary` instance from inline text with a specific language code.
    ///
    /// # Example
    ///
    /// ```rust
    ///
    /// use golem_rust::agentic::{UnstructuredBinary};
    /// use golem_rust::Allowed;
    ///
    /// let text: UnstructuredBinary<MyLanguage> =
    ///    UnstructuredBinary::from_inline("foo".to_string(), MimeTypes::ApplicationJson);
    ///
    ///
    /// #[derive(AllowedMimeType)]
    /// enum MimeTypes {
    ///     #[code = "application/json"]
    ///     ApplicationJson,
    ///     Fr,
    /// }
    /// ```
    pub fn from_inline(data: Vec<u8>, mime_type: T) -> UnstructuredBinary<T> {
        UnstructuredBinary::Inline {
            data,
            mime_type
        }
    }
}

impl UnstructuredBinary<AnyMimeType> {
    /// Create an `UnstructuredBinary` instance from a URL.
    ///
    /// # Example
    /// ```
    /// use golem_rust::agentic::UnstructuredBinary;
    /// let url_text = UnstructuredBinary::from_url_any("http://example.com".to_string());
    ///
    /// ```
    pub fn from_url_any(url: String) -> UnstructuredBinary<AnyMimeType> {
        UnstructuredBinary::Url(url)
    }
}

/// Trait for types representing allowed language codes.
///
/// Implement this trait for enums representing supported languages. Provides
/// conversion between enum variants and their string language codes.
/// Use the `#[derive(AllowedMimeType)]` macro to automatically implement this trait.
pub trait AllowedMimeType {
    fn all() -> &'static [&'static str];

    fn from_mime_type(mime_type: &str) -> Option<Self>
    where
        Self: Sized;

    fn to_string(&self) -> &'static str;
}

pub struct AnyMimeType;

impl AllowedMimeType for AnyMimeType {
    fn all() -> &'static [&'static str] {
        &[]
    }

    fn from_mime_type(_mime_type: &str) -> Option<Self> {
        None
    }

    fn to_string(&self) -> &'static str {
       ""
    }
}

impl<T: AllowedMimeType> Schema for UnstructuredBinary<T> {
    fn get_type() -> ElementSchema {
        let restrictions = if T::all().is_empty() {
            None
        } else {
            let restrictions = T::all()
                .iter()
                .map(|code| BinaryType {
                    mime_type: code.to_string(),
                })
                .collect::<Vec<_>>();

            Some(restrictions)
        };

        ElementSchema::UnstructuredBinary(BinaryDescriptor { restrictions })
    }

    fn to_element_value(self) -> Result<ElementValue, String> {
        match self {
            UnstructuredBinary::Inline {
                data,
                mime_type,
            } => {
                let mime_type = mime_type.to_string();

                Ok(ElementValue::UnstructuredBinary(BinaryReference::Inline(
                    BinarySource {
                        data: vec![],
                        binary_type: BinaryType {
                            mime_type: mime_type.to_string(),
                        },
                    },
                )))
            }

            UnstructuredBinary::Url(url) => {
                Ok(ElementValue::UnstructuredBinary(BinaryReference::Url(url)))
            }
        }
    }

    fn from_element_value(value: ElementValue, _schema: ElementSchema) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            ElementValue::ComponentModel(_) => {
                Err("Expected UnstructuredBinary ElementValue, got ComponentModel".to_string())
            }
            ElementValue::UnstructuredBinary(binary_reference) => match binary_reference {
                BinaryReference::Url(url) => Ok(UnstructuredBinary::Url(url)),
                BinaryReference::Inline(binary_source) => {
                    let allowed = T::all().iter().map(|s| s.to_string()).collect::<Vec<_>>();

                    let mime_type = binary_source.binary_type.mime_type;

                    if !allowed.contains(&mime_type) {
                        return Err(format!(
                            "Mime type '{}' is not allowed. Allowed mime types: {:?}",
                            mime_type, allowed.join(",")
                        ));
                    }

                    let mime_type = T::from_mime_type(&mime_type).ok_or_else(
                        || format!("Failed to convert mime type '{}' to AllowedMimeType", mime_type
                    ))?;

                    Ok(UnstructuredBinary::Inline {
                        data: binary_source.data,
                        mime_type,
                    })
                }
            },
            ElementValue::UnstructuredText(_) => {
                Err("Expected UnstructuredBinary ElementValue, got UnstructuredBinary".to_string())
            }
        }
    }
}
