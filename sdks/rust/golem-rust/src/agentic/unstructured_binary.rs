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
use crate::golem_agentic::golem::agent::common::{
    BinaryDescriptor, BinaryReference, BinarySource, BinaryType, ElementSchema, ElementValue,
    WitValue,
};
use golem_wasm::Value;

/// Represents a binary value that can either be inline or a URL reference.
///
/// `MT` specifies the allowed language codes for inline text.
pub enum UnstructuredBinary<MT: AllowedMimeTypes> {
    Url(String),
    Inline { data: Vec<u8>, mime_type: MT },
}

impl<T: AllowedMimeTypes> UnstructuredBinary<T> {
    /// Create an `UnstructuredBinary` instance from inline data and a mime type.
    ///
    /// # Example
    ///
    /// ```rust
    ///
    /// use golem_rust::agentic::{UnstructuredBinary};
    /// use golem_rust::AllowedMimeTypes;
    ///
    /// let text: UnstructuredBinary<MimeTypes> =
    ///    UnstructuredBinary::from_inline(vec![1, 2], MimeTypes::ApplicationJson);
    ///
    ///
    /// #[derive(AllowedMimeTypes)]
    /// enum MimeTypes {
    ///     #[mime_type("application/json")]
    ///     ApplicationJson,
    ///     Fr,
    /// }
    /// ```
    pub fn from_inline(data: Vec<u8>, mime_type: T) -> UnstructuredBinary<T> {
        UnstructuredBinary::Inline { data, mime_type }
    }

    pub fn from_url(url: String) -> UnstructuredBinary<T> {
        UnstructuredBinary::Url(url)
    }

    pub fn from_wit_value(wit_value: WitValue) -> Result<Self, String> {
        let value = Value::from(wit_value);

        let (case_idx, case_value) = match value {
            Value::Variant {
                case_idx,
                case_value,
            } => (case_idx, case_value),
            _ => return Err("Expected Variant for UnstructuredBinary".into()),
        };

        match case_idx {
            0 => {
                let boxed = case_value.ok_or("Expected value for Url variant")?;
                match *boxed {
                    Value::String(url) => Ok(UnstructuredBinary::Url(url)),
                    _ => Err("Expected String for Url variant".into()),
                }
            }

            1 => {
                let boxed = case_value.ok_or("Expected value for Inline variant")?;

                let fields = match *boxed {
                    Value::Record(fields) => fields,
                    _ => return Err("Expected Record for Inline variant".into()),
                };

                if fields.len() < 2 {
                    return Err("Expected at least two fields in Inline variant".into());
                }

                let data_str = match &fields[0] {
                    Value::String(s) => s,
                    _ => return Err("Expected String for data field".into()),
                };
                let data_bytes = data_str.as_bytes().to_vec();

                let mime_type_val = match &fields[1] {
                    Value::Option(Some(inner)) => inner.as_ref(),
                    Value::Option(None) => {
                        return Err("Mime type restriction is required for inline binary".into())
                    }
                    _ => return Err("Expected Option for mime type restriction".into()),
                };

                let mime_type_record = match mime_type_val {
                    Value::Record(fields) => fields,
                    _ => return Err("Expected Record for mime type restriction".into()),
                };

                if mime_type_record.is_empty() {
                    return Err("Expected at least one field in mime type restriction".into());
                }

                let mime_code = match &mime_type_record[0] {
                    Value::String(code) => code,
                    _ => return Err("Expected String for mime type".into()),
                };

                let mime_type = T::from_string(mime_code)
                    .ok_or_else(|| format!("Failed to convert mime type '{}'", mime_code))?;

                Ok(UnstructuredBinary::Inline {
                    data: data_bytes,
                    mime_type,
                })
            }

            _ => Err("Unknown variant index for UnstructuredBinary".into()),
        }
    }
}

/// Trait for types representing allowed language codes.
///
/// Implement this trait for enums representing supported languages. Provides
/// conversion between enum variants and their string language codes.
/// Use the `#[derive(AllowedMimeTypes)]` macro to automatically implement this trait.
pub trait AllowedMimeTypes {
    fn all() -> &'static [&'static str];

    fn from_string(mime_type: &str) -> Option<Self>
    where
        Self: Sized;

    fn to_string(&self) -> String;
}

impl AllowedMimeTypes for String {
    fn all() -> &'static [&'static str] {
        &["*"] // in the schema, it would imply you can pass any mime type
    }

    fn from_string(mime_type: &str) -> Option<Self> {
        Some(mime_type.to_string())
    }

    fn to_string(&self) -> String {
        self.clone()
    }
}

impl<T: AllowedMimeTypes> Schema for UnstructuredBinary<T> {
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
            UnstructuredBinary::Inline { data, mime_type } => {
                let mime_type = mime_type.to_string();

                Ok(ElementValue::UnstructuredBinary(BinaryReference::Inline(
                    BinarySource {
                        data,
                        binary_type: BinaryType { mime_type },
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

    fn from_wit_value(
        wit_value: golem_wasm::WitValue,
        _schema: ElementSchema,
    ) -> Result<Self, String>
    where
        Self: Sized,
    {
        UnstructuredBinary::from_wit_value(wit_value)
    }
}
