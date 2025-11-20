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

use crate::agentic::{Schema, StructuredSchema, StructuredValue};
use crate::golem_agentic::golem::agent::common::{
    BinaryReference, ElementSchema, ElementValue, TextDescriptor, TextReference, TextSource,
    TextType, WitValue,
};
use golem_wasm::Value;

/// Represents a text value that can either be inline or a URL reference.
///
/// `LC` specifies the allowed language codes for inline text. Defaults to `AnyLanguage`,
/// which allows all languages.
pub enum UnstructuredText<LC: AllowedLanguages = AnyLanguage> {
    Url(String),
    Text {
        text: String,
        language_code: Option<LC>,
    },
}

impl<T: AllowedLanguages> UnstructuredText<T> {
    /// Create an `UnstructuredText` instance from inline text with a specific language code.
    ///
    /// # Example
    ///
    /// ```rust
    ///
    /// use golem_rust::agentic::{UnstructuredText};
    /// use golem_rust::AllowedLanguages;
    ///
    /// let text: UnstructuredText<MyLanguage> =
    ///    UnstructuredText::from_inline("foo".to_string(), MyLanguage::English);
    ///
    ///
    /// #[derive(AllowedLanguages)]
    /// enum MyLanguage {
    ///     #[code = "en"]
    ///     English,
    ///     Fr,
    /// }
    ///
    /// // Create an UnstructuredText instance with no specific language code
    /// let any_text: UnstructuredText =
    ///    UnstructuredText::from_inline_any("bar".to_string());
    /// ```
    pub fn from_inline(text: String, language_code: T) -> UnstructuredText<T> {
        UnstructuredText::Text {
            text: text.into(),
            language_code: Some(language_code),
        }
    }

    pub fn from_wit_value(value: WitValue) -> Result<UnstructuredText<T>, String> {
        let value = Value::from(value);

        match value {
            Value::Variant {
                case_value,
                case_idx,
            } => match case_idx {
                0 => {
                    if let Some(boxed_url) = case_value {
                        if let Value::String(url) = *boxed_url {
                            Ok(UnstructuredText::Url(url))
                        } else {
                            Err("Expected String for URL variant".to_string())
                        }
                    } else {
                        Err("Expected value for URL variant".to_string())
                    }
                }
                1 => {
                    if let Some(boxed_record) = case_value {
                        if let Value::Record(mut fields) = *boxed_record {
                            if fields.len() != 2 {
                                return Err("Expected 2 fields in Inline variant".to_string());
                            }

                            let text_value = fields.remove(0);
                            let lang_option_value = fields.remove(0);

                            let text = if let Value::String(t) = text_value {
                                t
                            } else {
                                return Err("Expected String for text field".to_string());
                            };

                            let language_code = match lang_option_value {
                                Value::Option(Some(boxed_lang_record)) => {
                                    if let Value::Record(lang_fields) = *boxed_lang_record {
                                        if lang_fields.len() != 1 {
                                            return Err("Expected 1 field in language code record"
                                                .to_string());
                                        }

                                        if let Value::String(code) = &lang_fields[0] {
                                            T::from_language_code(code)
                                        } else {
                                            return Err(
                                                "Expected String for language code".to_string()
                                            );
                                        }
                                    } else {
                                        return Err("Expected Record for language code".to_string());
                                    }
                                }
                                _ => None,
                            };

                            Ok(UnstructuredText::Text {
                                text,
                                language_code,
                            })
                        } else {
                            Err("Expected Record for Inline variant".to_string())
                        }
                    } else {
                        Err("Expected value for Inline variant".to_string())
                    }
                }
                _ => Err("Unknown variant index for UnstructuredText".to_string()),
            },
            _ => Err("Expected Variant value for UnstructuredText".to_string()),
        }
    }
}

impl UnstructuredText<AnyLanguage> {
    /// Create an `UnstructuredText` instance from a URL.
    ///
    /// # Example
    /// ```
    /// use golem_rust::agentic::UnstructuredText;
    /// let url_text = UnstructuredText::from_url_any("http://example.com".to_string());
    ///
    /// ```
    pub fn from_url_any(url: String) -> UnstructuredText<AnyLanguage> {
        UnstructuredText::Url(url.into())
    }

    /// Create an `UnstructuredText` instance from inline text without specifying a language code.
    ///
    /// # Example
    ///
    /// ```
    /// use golem_rust::agentic::UnstructuredText;
    /// let text = UnstructuredText::from_inline_any("Hello, world!".to_string());
    /// ```
    ///
    pub fn from_inline_any(text: String) -> UnstructuredText<AnyLanguage> {
        UnstructuredText::Text {
            text: text.into(),
            language_code: None,
        }
    }
}

/// Trait for types representing allowed language codes.
///
/// Implement this trait for enums representing supported languages. Provides
/// conversion between enum variants and their string language codes.
/// Use the `#[derive(AllowedLanguages)]` macro to automatically implement this trait.
pub trait AllowedLanguages {
    fn all() -> &'static [&'static str];

    fn from_language_code(code: &str) -> Option<Self>
    where
        Self: Sized;

    fn to_language_code(&self) -> &'static str;
}

pub struct AnyLanguage;

impl AllowedLanguages for AnyLanguage {
    fn all() -> &'static [&'static str] {
        &[]
    }

    fn from_language_code(_code: &str) -> Option<Self> {
        None
    }

    fn to_language_code(&self) -> &'static str {
        ""
    }
}

impl<T: AllowedLanguages> Schema for UnstructuredText<T> {
    fn get_type() -> StructuredSchema {
        let restrictions = if T::all().is_empty() {
            None
        } else {
            let restrictions = T::all()
                .iter()
                .map(|code| TextType {
                    language_code: code.to_string(),
                })
                .collect::<Vec<_>>();

            Some(restrictions)
        };

        StructuredSchema::Default(ElementSchema::UnstructuredText(TextDescriptor {
            restrictions,
        }))
    }

    fn to_structured_value(self) -> Result<StructuredValue, String> {
        match self {
            UnstructuredText::Text {
                text,
                language_code,
            } => {
                let text_type = language_code.map(|code| TextType {
                    language_code: code.to_language_code().to_string(),
                });

                Ok(StructuredValue::Default(ElementValue::UnstructuredText(
                    TextReference::Inline(TextSource {
                        data: text,
                        text_type,
                    }),
                )))
            }

            UnstructuredText::Url(url) => Ok(StructuredValue::Default(
                ElementValue::UnstructuredText(TextReference::Url(url)),
            )),
        }
    }

    fn from_wit_value(wit_value: WitValue, _schema: StructuredSchema) -> Result<Self, String>
    where
        Self: Sized,
    {
        UnstructuredText::from_wit_value(wit_value)
    }

    fn from_unstructured_value(
        value: StructuredValue,
        _schema: StructuredSchema,
    ) -> Result<Self, String>
    where
        Self: Sized,
    {
        let element_value = match value {
            StructuredValue::Default(value) => Ok(value),
            StructuredValue::Multimodal(_) => {
                Err("input mismatch. expected default value, found multimodal".to_string())
            }
        }?;

        match element_value {
            ElementValue::ComponentModel(_) => {
                Err("Expected UnstructuredText ElementValue, got ComponentModel".to_string())
            }
            ElementValue::UnstructuredText(text_reference) => match text_reference {
                TextReference::Url(url) => Ok(UnstructuredText::Url(url)),
                TextReference::Inline(text_source) => {
                    let allowed = T::all().iter().map(|s| s.to_string()).collect::<Vec<_>>();

                    let language_code = match text_source.text_type {
                        Some(text_type) => {
                            if !allowed.is_empty() && !allowed.contains(&text_type.language_code) {
                                return Err(format!(
                                    "Language code '{}' is not allowed. Allowed codes: {}",
                                    text_type.language_code,
                                    allowed.join(", ")
                                ));
                            }

                            T::from_language_code(&text_type.language_code)
                        }
                        None => None,
                    };

                    Ok(UnstructuredText::Text {
                        text: text_source.data,
                        language_code,
                    })
                }
            },
            ElementValue::UnstructuredBinary(_) => {
                Err("Expected UnstructuredText ElementValue, got UnstructuredBinary".to_string())
            }
        }
    }

    fn to_wit_value(self) -> Result<golem_wasm::WitValue, String>
    where
        Self: Sized,
    {
        let value_type = self.to_structured_value()?;

        let element_value_result = match value_type {
            StructuredValue::Default(element_value) => Ok(element_value),
            StructuredValue::Multimodal(_) => {
                Err("Expected element value but found multimodal".to_string())
            }
        };

        let element_value = element_value_result?;

        match element_value {
            ElementValue::ComponentModel(wit_value) => Ok(wit_value),
            ElementValue::UnstructuredBinary(binary_reference) => Err(format!(
                "Expected ComponentModel value, got UnstructuredBinary: {:?}",
                binary_reference
            )),
            ElementValue::UnstructuredText(text_reference) => match text_reference {
                TextReference::Url(url) => {
                    let value = Value::Variant {
                        case_idx: 0,
                        case_value: Some(Box::new(Value::String(url))),
                    };

                    Ok(golem_wasm::WitValue::from(value))
                }

                TextReference::Inline(text_source) => {
                    let restriction_record = text_source.text_type.map(|text_type| {
                        Box::new(Value::Record(vec![Value::String(text_type.language_code)]))
                    });

                    let value = Value::Variant {
                        case_idx: 1,
                        case_value: Some(Box::new(Value::Record(vec![
                            Value::String(text_source.data),
                            Value::Option(restriction_record),
                        ]))),
                    };

                    Ok(golem_wasm::WitValue::from(value))
                }
            },
        }
    }
}
