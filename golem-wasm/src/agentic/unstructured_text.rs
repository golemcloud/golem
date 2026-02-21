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

use std::fmt::Debug;

/// Represents a text value that can either be inline or a URL reference.
///
/// `LC` specifies the allowed language codes for inline text. Defaults to `AnyLanguage`,
/// which allows all languages.
#[derive(Debug, Clone)]
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
            text,
            language_code: Some(language_code),
        }
    }

    #[cfg(any(feature = "host", feature = "stub"))]
    pub fn from_wit_value(value: crate::WitValue) -> Result<UnstructuredText<T>, String> {
        let value = crate::Value::from(value);

        match value {
            crate::Value::Variant {
                case_value,
                case_idx,
            } => match case_idx {
                0 => {
                    if let Some(boxed_url) = case_value {
                        if let crate::Value::String(url) = *boxed_url {
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
                        if let crate::Value::Record(mut fields) = *boxed_record {
                            if fields.len() != 2 {
                                return Err("Expected 2 fields in Inline variant".to_string());
                            }

                            let text_value = fields.remove(0);
                            let lang_option_value = fields.remove(0);

                            let text = if let crate::Value::String(t) = text_value {
                                t
                            } else {
                                return Err("Expected String for text field".to_string());
                            };

                            let language_code = match lang_option_value {
                                crate::Value::Option(Some(boxed_lang_record)) => {
                                    if let crate::Value::Record(lang_fields) = *boxed_lang_record {
                                        if lang_fields.len() != 1 {
                                            return Err("Expected 1 field in language code record"
                                                .to_string());
                                        }

                                        if let crate::Value::String(code) = &lang_fields[0] {
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
        UnstructuredText::Url(url)
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
            text,
            language_code: None,
        }
    }
}

/// Trait for types representing allowed language codes.
///
/// Implement this trait for enums representing supported languages. Provides
/// conversion between enum variants and their string language codes.
/// Use the `#[derive(AllowedLanguages)]` macro to automatically implement this trait.
pub trait AllowedLanguages: Debug + Clone {
    fn all() -> &'static [&'static str];

    fn from_language_code(code: &str) -> Option<Self>
    where
        Self: Sized;

    fn to_language_code(&self) -> &'static str;
}

#[derive(Debug, Clone)]
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
